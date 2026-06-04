// IP 测速选择器
//
// 对 DoH 返回的多个 IP 做 TCP 连接测速，选延迟最低的
// IPv6 自动降级：连续失败多次后暂时禁用
//
// 惰性淘汰缓存：get 时自动查 TTL，不用主动 evict
//
// 性能优化：
// - SmallVec 避免小规模 IP 列表的堆分配
// - IPv4/IPv6 分区用栈上缓冲区

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant};

use futures::stream::{FuturesUnordered, StreamExt};
use smallvec::SmallVec;
use tokio::net::TcpStream;
use tokio::sync::Semaphore;
use tracing::{debug, info, trace, warn};

use crate::core::config::DnsConfig;
use crate::utils::evictable_cache::EvictableCache;

// SmallVec 栈上分配，避免小列表堆分配
type IpSmallVec = SmallVec<[IpAddr; 8]>;

struct IpProbeResult {
    ip: IpAddr,
    latency: Duration,
    reachable: bool,
}

#[derive(Debug, Clone)]
struct BestIpValue {
    ip: IpAddr,
}

#[derive(Debug, Clone)]
pub enum IpSelectResult {
    // 找到可达的最佳 IP
    BestIp(IpAddr),
    // 全挂了，返回所有候选 IP 供下游回退
    AllUnreachable(Vec<IpAddr>),
}

struct Ipv6Status {
    // 连续失败次数
    consecutive_failures: AtomicU32,
    // 禁用截止时间（None 表示未禁用）
    disabled_until: RwLock<Option<Instant>>,
}

impl Ipv6Status {
    fn new() -> Self {
        Self {
            consecutive_failures: AtomicU32::new(0),
            disabled_until: RwLock::new(None),
        }
    }
}

pub struct IpProber {
    // 测速超时
    probe_timeout: Duration,
    // 最大并发测速数（Arc 用于 async 闭包共享）
    max_concurrent_arc: std::sync::Arc<Semaphore>,
    // 最佳 IP 缓存（带 LRU 淘汰和 TTL）
    best_ip_cache: EvictableCache<String, BestIpValue>,
    // IPv6 状态跟踪
    ipv6_status: Ipv6Status,
    // IPv6 连续失败阈值（0 表示不禁用）
    ipv6_failure_threshold: u32,
    // IPv6 禁用恢复时间
    ipv6_recovery_duration: Duration,
    // 是否启用 IPv6 探测
    ipv6_enabled: bool,
}

async fn probe_single_inner(ip: IpAddr, probe_timeout: Duration) -> Duration {
    let addr = SocketAddr::new(ip, 443);
    let start = Instant::now();

    match tokio::time::timeout(probe_timeout, TcpStream::connect(addr)).await {
        Ok(Ok(_)) => {
            let latency = start.elapsed();
            trace!("Probe {} -> {:?} (reachable)", ip, latency);
            latency
        }
        Ok(Err(e)) => {
            trace!("Probe {} -> failed: {}", ip, e);
            Duration::MAX
        }
        Err(_) => {
            trace!("Probe {} -> timeout", ip);
            Duration::MAX
        }
    }
}

impl IpProber {
    pub fn new(config: &DnsConfig) -> Self {
        let cache_ttl = Duration::from_secs(config.cache_ttl_secs);
        Self {
            probe_timeout: Duration::from_secs(config.probe_timeout_secs),
            max_concurrent_arc: std::sync::Arc::new(Semaphore::new(config.max_concurrent_probes)),
            best_ip_cache: EvictableCache::with_ttl(
                config.max_cache_entries.unwrap_or(500),
                "ip_prober",
                cache_ttl,
            ),
            ipv6_status: Ipv6Status::new(),
            ipv6_failure_threshold: config.ipv6_failure_threshold,
            ipv6_recovery_duration: Duration::from_secs(config.ipv6_recovery_secs),
            ipv6_enabled: config.ipv6_enabled,
        }
    }

    // 检查 IPv6 是否应该探测
    fn should_probe_ipv6(&self) -> bool {
        // 全局禁用 IPv6
        if !self.ipv6_enabled {
            return false;
        }

        // 阈值为 0 表示不禁用
        if self.ipv6_failure_threshold == 0 {
            return true;
        }

        let disabled_until = self.ipv6_status.disabled_until.read().unwrap();
        if let Some(until) = *disabled_until {
            if Instant::now() < until {
                return false;
            }
        }
        true
    }

    // 记录 IPv6 探测结果
    fn record_ipv6_probe_result(&self, success: bool) {
        if self.ipv6_failure_threshold == 0 {
            return;
        }

        if success {
            // 成功，重置失败计数
            let prev = self.ipv6_status.consecutive_failures.swap(0, Ordering::Relaxed);
            if prev > 0 {
                debug!("IPv6 探测成功，重置失败计数（之前: {}）", prev);
            }
            // 清除禁用状态
            let mut disabled_until = self.ipv6_status.disabled_until.write().unwrap();
            if disabled_until.is_some() {
                *disabled_until = None;
                info!("IPv6 恢复探测");
            }
        } else {
            // 失败，增加计数
            let failures = self.ipv6_status.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
            debug!("IPv6 探测失败，连续失败次数: {}/{}", failures, self.ipv6_failure_threshold);

            // 达到阈值，禁用 IPv6
            if failures >= self.ipv6_failure_threshold {
                let mut disabled_until = self.ipv6_status.disabled_until.write().unwrap();
                let recovery_secs = self.ipv6_recovery_duration.as_secs();
                *disabled_until = Some(Instant::now() + self.ipv6_recovery_duration);
                warn!(
                    "IPv6 连续失败 {} 次，暂时禁用 {} 秒",
                    failures, recovery_secs
                );
            }
        }
    }

    // 对多个 IP 测速，返回按延迟从低到高排序的 IP 列表
    //
    // 智能快速返回：
    // - 并发探测所有 IP（受 max_concurrent 限制）
    // - 收集到 3 个可达结果后立即返回（保证有足够候选选最优）
    // - 或等待超过 200ms 后返回已收集的结果（避免等太久）
    // - 如果 IP 数量 <= 3，等待全部完成
    pub async fn select_ranked_ips(
        &self,
        domain: &str,
        ips: Vec<IpAddr>,
    ) -> Vec<IpAddr> {
        if ips.is_empty() {
            return vec![];
        }

        // 手动分区到 SmallVec，避免 iter().partition() 的堆分配
        let mut ipv4_ips: IpSmallVec = SmallVec::new();
        let mut ipv6_ips: IpSmallVec = SmallVec::new();
        for ip in &ips {
            if ip.is_ipv4() {
                ipv4_ips.push(*ip);
            } else {
                ipv6_ips.push(*ip);
            }
        }

        let probe_ipv6 = self.should_probe_ipv6();
        let must_probe_ipv6 = ipv4_ips.is_empty() && !ipv6_ips.is_empty();

        // 构建要探测的 IP 列表
        let ips_to_probe: IpSmallVec = if probe_ipv6 || must_probe_ipv6 {
            // 需要探测 IPv6，用原始完整列表
            SmallVec::from_vec(ips)
        } else {
            // 只探测 IPv4
            ipv4_ips
        };

        let total = ips_to_probe.len();

        // 智能返回阈值：IP 数量 <= 3 时等待全部，否则收集 3 个即返回
        const MIN_RESULTS_FOR_FAST_RETURN: usize = 3;
        const FAST_RETURN_TIMEOUT_MS: u64 = 200;

        let fast_return_enabled = total > MIN_RESULTS_FOR_FAST_RETURN;
        let deadline = Instant::now() + Duration::from_millis(FAST_RETURN_TIMEOUT_MS);

        // 用 Arc 共享 Semaphore，避免闭包所有权问题
        let sem = std::sync::Arc::clone(&self.max_concurrent_arc);
        let probe_timeout = self.probe_timeout;

        let mut tasks: FuturesUnordered<_> = ips_to_probe
            .into_iter()
            .map(|ip| {
                let sem = sem.clone();
                async move {
                    let _permit = sem.acquire().await.ok();
                    (ip, probe_single_inner(ip, probe_timeout).await)
                }
            })
            .collect();

        let mut results: Vec<(IpAddr, Duration)> = Vec::with_capacity(total);

        // 流式收集，满足条件即退出
        while let Some(result) = tasks.next().await {
            let (ip, latency) = result;
            if latency < self.probe_timeout {
                results.push((ip, latency));
            }

            // 快速返回条件：已收集足够结果 且 超过截止时间
            if fast_return_enabled && results.len() >= MIN_RESULTS_FOR_FAST_RETURN && Instant::now() >= deadline {
                debug!("快速返回: 已收集 {} 个可达 IP (共 {} 个待探测)", results.len(), total);
                break;
            }
        }

        // 如果快速返回时结果不足，继续等待剩余任务
        if results.len() < MIN_RESULTS_FOR_FAST_RETURN && !tasks.is_empty() {
            debug!("继续等待剩余探测 (当前 {} 个可达)", results.len());
            while let Some((ip, latency)) = tasks.next().await {
                if latency < self.probe_timeout {
                    results.push((ip, latency));
                }
            }
        }

        if probe_ipv6 && !ipv6_ips.is_empty() {
            let ipv6_reachable = results.iter().any(|(ip, _)| ip.is_ipv6());
            self.record_ipv6_probe_result(ipv6_reachable);
        }

        if results.is_empty() {
            warn!("所有 IP 不可达 (共 {} 个): {}", total, domain);
            return vec![];
        }

        // 按延迟排序
        results.sort_by_key(|(_, lat)| *lat);
        let ranked: Vec<IpAddr> = results.into_iter().map(|(ip, _)| ip).collect();

        self.best_ip_cache.insert(domain.to_string(), BestIpValue { ip: ranked[0] });
        debug!("IP 排名: {} -> [{}]", domain, ranked.iter()
            .map(|ip| ip.to_string()).collect::<Vec<_>>().join(", "));

        ranked
    }

    // 强制重测，返回排序后的 IP 列表
    pub async fn force_reprobe_ranked(
        &self,
        domain: &str,
        ips: Vec<IpAddr>,
    ) -> Vec<IpAddr> {
        self.best_ip_cache.remove(domain);
        self.select_ranked_ips(domain, ips).await
    }

    // 对多个 IP 测速，返回延迟最低的可达 IP
    pub async fn select_best_ip(
        &self,
        domain: &str,
        ips: Vec<IpAddr>,
    ) -> anyhow::Result<IpAddr> {
        if ips.is_empty() {
            anyhow::bail!("没有可用的 IP 地址: {}", domain);
        }

        // 惰性淘汰：get 时自动查 TTL
        if let Some(entry) = self.best_ip_cache.get(domain) {
            debug!("IP 测速缓存命中: {} -> {}", domain, entry.ip);
            return Ok(entry.ip);
        }

        debug!("开始测速 {} 个 IP for {}", ips.len(), domain);

        // 并发测速
        let probe_results = self.probe_all_with_ipv6_handling(&ips).await;

        // 选择最佳
        match self.pick_best(&probe_results)? {
            IpSelectResult::BestIp(best_ip) => {
                self.best_ip_cache.insert(
                    domain.to_string(),
                    BestIpValue { ip: best_ip },
                );

                debug!("最佳 IP: {} -> {}", domain, best_ip);
                Ok(best_ip)
            }
            IpSelectResult::AllUnreachable(all_ips) => {
                warn!(
                    "所有 IP 不可达 (共 {} 个)，不缓存结果，交由下游回退机制处理: {}",
                    all_ips.len(),
                    domain
                );
                anyhow::ensure!(!all_ips.is_empty(), "DNS 解析返回空 IP 列表: {}", domain);
                Ok(all_ips[0])
            }
        }
    }

    // 对多个 IP 测速，返回详细的选择结果（包含回退信息）
    pub async fn select_best_ip_detailed(
        &self,
        domain: &str,
        ips: Vec<IpAddr>,
    ) -> anyhow::Result<IpSelectResult> {
        if ips.is_empty() {
            anyhow::bail!("没有可用的 IP 地址: {}", domain);
        }

        // 惰性淘汰：get 时自动查 TTL
        if let Some(entry) = self.best_ip_cache.get(domain) {
            debug!("IP 测速缓存命中: {} -> {}", domain, entry.ip);
            return Ok(IpSelectResult::BestIp(entry.ip));
        }

        debug!("开始测速 {} 个 IP for {}", ips.len(), domain);

        // 并发测速
        let probe_results = self.probe_all_with_ipv6_handling(&ips).await;

        // 选择最佳
        let result = self.pick_best(&probe_results)?;

        if let IpSelectResult::BestIp(best_ip) = &result {
            self.best_ip_cache.insert(
                domain.to_string(),
                BestIpValue { ip: *best_ip },
            );

            debug!("最佳 IP: {} -> {}", domain, best_ip);
        } else {
            warn!(
                "所有 IP 不可达 (共 {} 个)，不缓存结果，交由下游回退机制处理: {}",
                probe_results.len(),
                domain
            );
        }

        Ok(result)
    }

    // 强制重测
    pub async fn force_reprobe(
        &self,
        domain: &str,
        ips: Vec<IpAddr>,
    ) -> anyhow::Result<IpAddr> {
        self.best_ip_cache.remove(domain);
        self.select_best_ip(domain, ips).await
    }

    // 强制重测，返回详细结果
    pub async fn force_reprobe_detailed(
        &self,
        domain: &str,
        ips: Vec<IpAddr>,
    ) -> anyhow::Result<IpSelectResult> {
        self.best_ip_cache.remove(domain);
        self.select_best_ip_detailed(domain, ips).await
    }

    // 并发测速所有 IP，处理 IPv6 状态
    // 用 SmallVec 避免小规模 IP 列表的堆分配
    async fn probe_all_with_ipv6_handling(&self, ips: &[IpAddr]) -> Vec<IpProbeResult> {
        // 手动分区到 SmallVec
        let mut ipv4_ips: IpSmallVec = SmallVec::new();
        let mut ipv6_ips: IpSmallVec = SmallVec::new();
        for ip in ips {
            if ip.is_ipv4() {
                ipv4_ips.push(*ip);
            } else {
                ipv6_ips.push(*ip);
            }
        }

        // 检查是否应该探测 IPv6
        let probe_ipv6 = self.should_probe_ipv6();

        // 如果只有 IPv6 地址，即使禁用也必须探测
        let must_probe_ipv6 = ipv4_ips.is_empty() && !ipv6_ips.is_empty();

        // 保存长度用于日志（避免 move 后无法访问）
        let ipv4_count = ipv4_ips.len();
        let ipv6_count = ipv6_ips.len();

        // 构建要探测的 IP 列表
        let ips_to_probe: IpSmallVec = if probe_ipv6 || must_probe_ipv6 {
            SmallVec::from_slice(ips)
        } else {
            ipv4_ips
        };

        debug!(
            "探测 {} 个 IP (IPv4: {}, IPv6: {}{}{})",
            ips_to_probe.len(),
            ipv4_count,
            ipv6_count,
            if !probe_ipv6 && !must_probe_ipv6 { " [已禁用]" } else { "" },
            if must_probe_ipv6 { " [仅 IPv6，强制探测]" } else { "" },
        );

        // 并发探测
        let futures: Vec<_> = ips_to_probe.iter().map(|ip| self.probe_single(*ip)).collect();
        let results = futures::future::join_all(futures).await;

        let probe_results: Vec<IpProbeResult> = ips_to_probe
            .iter()
            .zip(results)
            .map(|(ip, latency)| IpProbeResult {
                ip: *ip,
                latency,
                reachable: latency < self.probe_timeout,
            })
            .collect();

        // 更新 IPv6 状态
        if probe_ipv6 && ipv6_count > 0 {
            let ipv6_results: Vec<_> = probe_results.iter().filter(|r| r.ip.is_ipv6()).collect();
            let any_success = ipv6_results.iter().any(|r| r.reachable);
            self.record_ipv6_probe_result(any_success);
        }

        probe_results
    }

    // 测速单个 IP（TCP 连接延迟）
    async fn probe_single(&self, ip: IpAddr) -> Duration {
        let _permit = self.max_concurrent_arc.acquire().await.ok();
        probe_single_inner(ip, self.probe_timeout).await
    }

    // 选择延迟最低的可达 IP
    fn pick_best(&self, results: &[IpProbeResult]) -> anyhow::Result<IpSelectResult> {
        let reachable: Vec<_> = results.iter().filter(|r| r.reachable).collect();

        if reachable.is_empty() {
            let all_ips: Vec<IpAddr> = results.iter().map(|r| r.ip).collect();
            return Ok(IpSelectResult::AllUnreachable(all_ips));
        }

        let best = reachable
            .iter()
            .min_by_key(|r| r.latency)
            .ok_or_else(|| anyhow::anyhow!("没有可达的 IP"))?;

        Ok(IpSelectResult::BestIp(best.ip))
    }

    // 清掉指定域名的缓存
    pub fn invalidate(&self, domain: &str) {
        self.best_ip_cache.remove(domain);
    }

    // 清掉所有缓存
    pub fn invalidate_all(&self) {
        self.best_ip_cache.clear();
    }
}

