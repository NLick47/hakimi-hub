// DNS 解析器
// 支持系统解析、DoH、指定 IP，带测速和缓存
// 惰性淘汰：get 时自动查 TTL，不用主动 evict

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use futures::stream::FuturesUnordered;
use futures::StreamExt;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

use crate::core::config::DnsConfig;
use crate::dns::doh_client::{DohClient, DohResolveResult};
use crate::dns::ip_prober::IpProber;
use crate::dns::{filter_ips, IpFilterLevel};
use crate::utils::evictable_cache::EvictableCache;

// 缓存值：按延迟排好序的 IP 列表（最快的在前）
#[derive(Debug, Clone)]
struct CacheValue {
    ips: Vec<IpAddr>,
}

// DNS 解析器，整合 DoH + 测速 + 缓存
pub struct DnsResolver {
    doh_client: Arc<DohClient>,
    ip_prober: IpProber,
    cache: EvictableCache<String, CacheValue>,
    probe_interval: Duration,
}

impl DnsResolver {
    pub fn new(config: &DnsConfig) -> anyhow::Result<Self> {
        let doh_client = Arc::new(DohClient::new(config.doh_endpoints.clone(), config.dns_mapping.clone()));

        doh_client.preresolve_doh_hosts();

        let max_entries = config.max_cache_entries.unwrap_or(500);
        let cache_ttl = Duration::from_secs(config.cache_ttl_secs);
        let probe_interval = Duration::from_secs(config.probe_interval_secs);

        Ok(Self {
            doh_client,
            ip_prober: IpProber::new(config),
            cache: EvictableCache::with_ttl(max_entries, "dns_cache", cache_ttl),
            probe_interval,
        })
    }

    // 解析域名，返回测速后最快的 IP
    pub async fn resolve(&self, domain: &str) -> anyhow::Result<IpAddr> {
        // 惰性淘汰：get 时自动查 TTL
        if let Some(entry) = self.cache.get(domain) {
            debug!("DNS 缓存命中: {} -> {}", domain, entry.ips[0]);
            return Ok(entry.ips[0]);
        }

        let DohResolveResult { ips, doh_servers } = self.doh_client.resolve(domain).await?;
        if ips.is_empty() {
            anyhow::bail!("DNS 解析返回空结果: {}", domain);
        }

        // 过滤无效 IP，用 PublicOnly 顺带过滤私有 IP（防污染）
        // 全部被过滤会自动回退
        let filtered_ips = filter_ips(&ips, IpFilterLevel::PublicOnly);
        if filtered_ips.len() != ips.len() {
            debug!(
                "IP 过滤: {} -> {} 个 IP（过滤掉 {} 个无效/私有 IP）",
                domain,
                filtered_ips.len(),
                ips.len() - filtered_ips.len()
            );
        }

        // 直接传所有权，省一次 clone
        let ranked = self.ip_prober.select_ranked_ips(domain, filtered_ips).await;

        let doh_info = if doh_servers.is_empty() {
            "DoH".to_string()
        } else {
            format!("DoH:{}", doh_servers.join(","))
        };

        if ranked.is_empty() {
            // 测速全挂，回退到原始 IP
            warn!(
                "DNS 解析: {} 所有 IP 不可达，尝试原始列表 (via {})",
                domain, doh_info
            );
            if !ips.is_empty() {
                self.cache
                    .insert(domain.to_string(), CacheValue { ips: ips.clone() });
                return Ok(ips[0]);
            }
            anyhow::bail!("DNS 解析: {} 无可用 IP", domain);
        }

        let best = ranked[0];
        info!(
            "DNS 解析: {} -> {} (via {} + 测速, {} 个 IP)",
            domain,
            best,
            doh_info,
            ranked.len()
        );
        self.cache
            .insert(domain.to_string(), CacheValue { ips: ranked });
        Ok(best)
    }

    // 拿到所有候选 IP，已按延迟排好
    pub async fn get_candidates(&self, domain: &str) -> Vec<IpAddr> {
        // 惰性淘汰：get 时自动查 TTL
        if let Some(entry) = self.cache.get(domain) {
            return entry.ips;
        }

        match self.doh_client.resolve(domain).await {
            Ok(DohResolveResult { ips, .. }) => {
                if !ips.is_empty() {
                    let filtered_ips = filter_ips(&ips, IpFilterLevel::PublicOnly);
                    // 直接传所有权
                    let ranked = self.ip_prober.select_ranked_ips(domain, filtered_ips).await;
                    // 测速失败回退原始列表
                    let cached = if ranked.is_empty() { ips } else { ranked };
                    self.cache.insert(
                        domain.to_string(),
                        CacheValue {
                            ips: cached.clone(),
                        },
                    );
                    return cached;
                }
                vec![]
            }
            Err(e) => {
                warn!("获取候选 IP 失败: {} -> {}", domain, e);
                Vec::new()
            }
        }
    }

    pub async fn resolve_best(&self, domain: &str) -> anyhow::Result<IpAddr> {
        self.resolve(domain).await
    }

    pub fn probe_interval(&self) -> Duration {
        self.probe_interval
    }

    pub async fn reprobe_all(&self) {
        const MAX_DOMAINS: usize = 50;
        const MAX_CONCURRENT: usize = 4;

        let mut entries: Vec<_> = self
            .cache
            .dashmap()
            .iter()
            .map(|e| (e.key().clone(), e.last_accessed))
            .collect();
        entries.sort_by_key(|(_, t)| *t);
        entries.reverse();
        entries.truncate(MAX_DOMAINS);

        let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT));
        let mut tasks = FuturesUnordered::new();

        for (domain, _) in entries {
            let sem = semaphore.clone();
            tasks.push(async move {
                let _permit = sem.acquire().await.unwrap();
                let r = self.doh_client.resolve(&domain).await;
                match r {
                    Ok(DohResolveResult { ips, .. }) if !ips.is_empty() => {
                        let filtered_ips = filter_ips(&ips, IpFilterLevel::PublicOnly);
                        let ranked = self
                            .ip_prober
                            .force_reprobe_ranked(&domain, filtered_ips)
                            .await;
                        if !ranked.is_empty() {
                            self.cache
                                .insert(domain.clone(), CacheValue { ips: ranked });
                            debug!("定时测速更新: {}", domain);
                        }
                    }
                    Ok(_) => debug!("定时测速: {} 返回空结果", domain),
                    Err(e) => warn!("定时测速 DoH 查询失败 {}: {}", domain, e),
                }
            });
        }

        while tasks.next().await.is_some() {}
    }

    // 仅用国际 DoH 重解（国内 IP 全挂时回退）
    pub async fn resolve_international(&self, domain: &str) -> anyhow::Result<Vec<IpAddr>> {
        let DohResolveResult { ips, doh_servers } =
            self.doh_client.resolve_international(domain).await?;
        if ips.is_empty() {
            anyhow::bail!("国际 DoH 未返回任何 IP: {}", domain);
        }

        let doh_info = if doh_servers.is_empty() {
            "International-DoH".to_string()
        } else {
            format!("DoH:{}", doh_servers.join(","))
        };

        let filtered_ips = filter_ips(&ips, IpFilterLevel::PublicOnly);
        // 直接传所有权
        let ranked = self.ip_prober.select_ranked_ips(domain, filtered_ips).await;
        if ranked.is_empty() {
            warn!("国际 DoH: {} 所有 IP 不可达 (via {})", domain, doh_info);
            self.cache
                .insert(domain.to_string(), CacheValue { ips: ips.clone() });
            return Ok(ips);
        }

        info!(
            "国际回退: {} -> {} (via {}, {} 个 IP)",
            domain,
            ranked[0],
            doh_info,
            ranked.len()
        );
        self.cache.insert(
            domain.to_string(),
            CacheValue {
                ips: ranked.clone(),
            },
        );
        Ok(ranked)
    }

    // 清掉指定域名的缓存
    pub fn invalidate(&self, domain: &str) {
        self.cache.remove(domain);
        self.ip_prober.invalidate(domain);
    }

    // 清掉所有缓存
    pub fn invalidate_all(&self) {
        self.cache.clear();
        self.ip_prober.invalidate_all();
    }

    pub fn doh_client(&self) -> Arc<DohClient> {
        self.doh_client.clone()
    }
}
