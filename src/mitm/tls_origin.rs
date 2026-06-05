use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::stream::{FuturesUnordered, StreamExt};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tracing::{debug, info, warn};

use crate::dns::resolver::DnsResolver;
use crate::mitm::sni_spoof::SniSpoofConnector;

// Happy Eyeballs 参数
const HAPPY_EYEBALLS_INITIAL_DELAY: Duration = Duration::from_millis(250);
const HAPPY_EYEBALLS_SUBSEQUENT_DELAY: Duration = Duration::from_millis(100);

// 建立到真实目标的 TLS 连接
pub struct TlsOrigin {
    dns_resolver: Arc<DnsResolver>,
    connect_timeout: Duration,
    tls_config: Arc<rustls::ClientConfig>,
    sni_spoof_connector: SniSpoofConnector,
}

impl TlsOrigin {
    pub fn new(
        dns_resolver: Arc<DnsResolver>,
        _browser: String,
        _version: String,
        connect_timeout_secs: u64,
    ) -> Self {
        let tls_config = Arc::new(Self::build_shared_tls_config());
        let sni_spoof_connector =
            SniSpoofConnector::new(dns_resolver.clone(), connect_timeout_secs);

        Self {
            dns_resolver,
            connect_timeout: Duration::from_secs(connect_timeout_secs),
            tls_config,
            sni_spoof_connector,
        }
    }

    // 共享的 TLS 配置
    fn build_shared_tls_config() -> rustls::ClientConfig {
        let root_store = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };

        let mut config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        config.resumption = rustls::client::Resumption::default();

        config
    }

    // DNS 解析
    pub async fn resolve(&self, domain: &str) -> anyhow::Result<std::net::IpAddr> {
        self.dns_resolver.resolve_best(domain).await
    }

    // 连接超时（秒）
    pub fn connect_timeout(&self) -> u64 {
        self.connect_timeout.as_secs()
    }

    // 建 TLS 连接
    pub async fn connect(&self, domain: &str) -> anyhow::Result<TlsStream<TcpStream>> {
        debug!("TLS 源端: 正在连接 {}", domain);

        let ip = self.dns_resolver.resolve_best(domain).await?;
        let addr = std::net::SocketAddr::new(ip, 443);

        debug!("TLS 源端: 解析 {} -> {}", domain, addr);

        let tcp_stream = TcpStream::connect(addr).await?;
        tcp_stream.set_nodelay(true)?;

        let connector = tokio_rustls::TlsConnector::from(self.tls_config.clone());
        let tls_stream = connector
            .connect(
                rustls::pki_types::ServerName::try_from(domain.to_string())?,
                tcp_stream,
            )
            .await?;

        debug!("TLS 连接已建立: {}", domain);

        Ok(tls_stream)
    }

    // 建 TLS 连接，支持多 IP 竞速 + 国际 DoH 回退 (RFC 8305 Happy Eyeballs v2)
    pub async fn connect_with_fallback(
        &self,
        domain: &str,
    ) -> anyhow::Result<TlsStream<TcpStream>> {
        let candidates = self.dns_resolver.get_candidates(domain).await;

        if candidates.is_empty() {
            anyhow::bail!("没有可用的候选 IP: {}", domain);
        }

        match self.run_happy_eyeballs(domain, &candidates).await {
            Ok(stream) => return Ok(stream),
            Err(first_err) => {
                debug!(
                    "国内 IP 全部失败 for {}, 尝试国际 DoH: {}",
                    domain, first_err
                );
            }
        }

        self.dns_resolver.invalidate(domain);
        let international_ips = self.dns_resolver.resolve_international(domain).await?;
        if international_ips.is_empty() {
            anyhow::bail!("国际 DoH 也未返回 IP: {}", domain);
        }

        info!(
            "国际回退: {} 获得 {} 个国际 IP 重试",
            domain,
            international_ips.len()
        );
        self.run_happy_eyeballs(domain, &international_ips).await
    }

    // 用预解析的 IP 列表建连，跳过 DNS
    pub async fn connect_with_ips(
        &self,
        domain: &str,
        ips: Vec<std::net::IpAddr>,
    ) -> anyhow::Result<TlsStream<TcpStream>> {
        if ips.is_empty() {
            // 没有预解析 IP，走正常流程
            return self.connect_with_fallback(domain).await;
        }

        debug!("使用预解析 IP 连接 {} ({} 个候选)", domain, ips.len());

        match self.run_happy_eyeballs(domain, &ips).await {
            Ok(stream) => return Ok(stream),
            Err(first_err) => {
                debug!(
                    "预解析 IP 全部失败 for {}, 尝试国际 DoH: {}",
                    domain, first_err
                );
            }
        }

        // 预解析 IP 全挂，试国际 DoH
        self.dns_resolver.invalidate(domain);
        let international_ips = self.dns_resolver.resolve_international(domain).await?;
        if international_ips.is_empty() {
            anyhow::bail!("国际 DoH 也未返回 IP: {}", domain);
        }

        info!(
            "国际回退: {} 获得 {} 个国际 IP 重试",
            domain,
            international_ips.len()
        );
        self.run_happy_eyeballs(domain, &international_ips).await
    }

    async fn run_happy_eyeballs(
        &self,
        domain: &str,
        candidates: &[std::net::IpAddr],
    ) -> anyhow::Result<TlsStream<TcpStream>> {
        debug!(
            "Happy Eyeballs 连接 {} (共 {} 个候选)",
            domain,
            candidates.len()
        );

        if candidates.len() == 1 {
            return self.try_tls_connect(domain, candidates[0]).await;
        }

        let domain = Arc::new(domain.to_string());
        let tls_config = self.tls_config.clone();
        let connect_timeout = self.connect_timeout;

        type PendingFuture =
            Pin<Box<dyn std::future::Future<Output = anyhow::Result<TlsStream<TcpStream>>> + Send>>;
        let mut pending: FuturesUnordered<PendingFuture> = FuturesUnordered::new();
        let mut next_idx: usize = 0;

        let d = domain.clone();
        let cfg = tls_config.clone();
        let ip = candidates[next_idx];
        pending.push(Box::pin(async move {
            Self::do_tls_connect(&d, &cfg, ip, connect_timeout).await
        }));
        next_idx += 1;

        let delay_timer = tokio::time::sleep(HAPPY_EYEBALLS_INITIAL_DELAY);
        tokio::pin!(delay_timer);
        let mut more_candidates = next_idx < candidates.len();

        loop {
            tokio::select! {
                Some(result) = pending.next() => {
                    match result {
                        Ok(stream) => {
                            debug!("Happy Eyeballs: 连接成功 (剩余 {} 个未完成)", pending.len());
                            return Ok(stream);
                        }
                        Err(e) => {
                            debug!("Happy Eyeballs: 候选连接失败: {}", e);
                            if !more_candidates && pending.is_empty() {
                                anyhow::bail!("所有 {} 个候选 IP 均连接失败: {}", candidates.len(), domain);
                            }
                        }
                    }
                }

                _ = &mut delay_timer, if more_candidates => {
                    let d = domain.clone();
                    let cfg = tls_config.clone();
                    let ip = candidates[next_idx];
                    pending.push(Box::pin(async move {
                        Self::do_tls_connect(&d, &cfg, ip, connect_timeout).await
                    }));
                    next_idx += 1;

                    if next_idx < candidates.len() {
                        delay_timer.as_mut().reset(tokio::time::Instant::now() + HAPPY_EYEBALLS_SUBSEQUENT_DELAY);
                    } else {
                        more_candidates = false;
                    }
                }
            }
        }
    }

    // TLS 连接（静态方法，用于 'static 上下文）
    async fn do_tls_connect(
        domain: &str,
        tls_config: &Arc<rustls::ClientConfig>,
        ip: std::net::IpAddr,
        connect_timeout: Duration,
    ) -> anyhow::Result<TlsStream<TcpStream>> {
        let addr = std::net::SocketAddr::new(ip, 443);
        debug!("尝试 TLS 连接 {} ({})", domain, addr);

        let tcp_stream = tokio::time::timeout(connect_timeout, TcpStream::connect(addr))
            .await
            .map_err(|_| {
                anyhow::anyhow!("TCP 连接超时 ({}, {}s)", ip, connect_timeout.as_secs())
            })??;

        if let Err(e) = tcp_stream.set_nodelay(true) {
            warn!("设置 TCP_NODELAY 失败: {}", e);
        }

        let connector = tokio_rustls::TlsConnector::from(tls_config.clone());
        let tls_stream = tokio::time::timeout(
            connect_timeout,
            connector.connect(
                rustls::pki_types::ServerName::try_from(domain.to_string())?,
                tcp_stream,
            ),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!("TLS 握手超时 ({}, {}s)", domain, connect_timeout.as_secs())
        })??;

        debug!("TLS 连接成功: {} (IP: {})", domain, ip);
        Ok(tls_stream)
    }

    // 单 IP 连接
    async fn try_tls_connect(
        &self,
        domain: &str,
        ip: std::net::IpAddr,
    ) -> anyhow::Result<TlsStream<TcpStream>> {
        let addr = std::net::SocketAddr::new(ip, 443);
        debug!("尝试 TLS 连接 {} ({})", domain, addr);

        let tcp_stream = tokio::time::timeout(self.connect_timeout, TcpStream::connect(addr))
            .await
            .map_err(|_| {
                anyhow::anyhow!("TCP 连接超时 ({}, {}s)", ip, self.connect_timeout.as_secs())
            })??;

        if let Err(e) = tcp_stream.set_nodelay(true) {
            warn!("设置 TCP_NODELAY 失败: {}", e);
        }

        let connector = tokio_rustls::TlsConnector::from(self.tls_config.clone());
        let tls_stream = tokio::time::timeout(
            self.connect_timeout,
            connector.connect(
                rustls::pki_types::ServerName::try_from(domain.to_string())?,
                tcp_stream,
            ),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "TLS 握手超时 ({}, {}s)",
                domain,
                self.connect_timeout.as_secs()
            )
        })??;

        debug!("TLS 连接成功: {} (IP: {})", domain, ip);
        Ok(tls_stream)
    }

    // TCP 隧道，支持 IP 回退 + 国际 DoH 兜底
    pub async fn connect_tcp_with_fallback(
        &self,
        domain: &str,
        port: u16,
    ) -> anyhow::Result<TcpStream> {
        let candidates = self.dns_resolver.get_candidates(domain).await;

        if candidates.is_empty() {
            anyhow::bail!("没有可用的候选 IP: {}", domain);
        }

        match self.try_connect_tcp(domain, port, &candidates).await {
            Ok(stream) => return Ok(stream),
            Err(e) => debug!("国内 IP 全部失败 {}:{}: {}, 尝试国际 DoH", domain, port, e),
        }

        self.dns_resolver.invalidate(domain);
        let international_ips = self.dns_resolver.resolve_international(domain).await?;
        if international_ips.is_empty() {
            anyhow::bail!("国际 DoH 也未返回 IP: {}", domain);
        }

        info!(
            "国际回退: {} 获得 {} 个国际 IP 重试 TCP",
            domain,
            international_ips.len()
        );
        self.try_connect_tcp(domain, port, &international_ips).await
    }

    async fn try_connect_tcp(
        &self,
        domain: &str,
        port: u16,
        ips: &[std::net::IpAddr],
    ) -> anyhow::Result<TcpStream> {
        let mut last_err: Option<anyhow::Error> = None;

        for ip in ips {
            let addr = std::net::SocketAddr::new(*ip, port);
            debug!("尝试 TCP 连接 {} ({})", domain, addr);

            match tokio::time::timeout(self.connect_timeout, TcpStream::connect(addr)).await {
                Ok(Ok(stream)) => {
                    let _ = stream.set_nodelay(true);
                    debug!("TCP 连接成功: {}:{} (IP: {})", domain, port, ip);
                    return Ok(stream);
                }
                Ok(Err(e)) => {
                    warn!("TCP 连接失败 {}:{} ({}): {}", domain, port, ip, e);
                    last_err = Some(e.into());
                }
                Err(_) => {
                    warn!(
                        "TCP 连接超时 {}:{} ({}, {}s)",
                        domain,
                        port,
                        ip,
                        self.connect_timeout.as_secs()
                    );
                    last_err = Some(anyhow::anyhow!(
                        "连接超时 ({}s)",
                        self.connect_timeout.as_secs()
                    ));
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("所有 IP 均连接失败: {}:{}", domain, port)))
    }

    // SNI 伪装连接
    pub async fn connect_with_sni_spoof(
        &self,
        real_domain: &str,
        fake_sni: &str,
    ) -> anyhow::Result<TlsStream<TcpStream>> {
        self.sni_spoof_connector
            .connect_with_fallback(real_domain, fake_sni)
            .await
    }

    pub fn sni_spoof_connector(&self) -> &SniSpoofConnector {
        &self.sni_spoof_connector
    }

    // DNS 解析器引用（用于预解析）
    pub fn dns_resolver(&self) -> &Arc<DnsResolver> {
        &self.dns_resolver
    }
}
