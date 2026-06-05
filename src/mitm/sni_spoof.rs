// SNI TLS 连接器
//
// 连到真实 IP 但发假的 SNI，跳过证书验证。

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::stream::{FuturesUnordered, StreamExt};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tracing::{debug, info};

use crate::dns::resolver::DnsResolver;

// Happy Eyeballs 参数
const HAPPY_EYEBALLS_INITIAL_DELAY: Duration = Duration::from_millis(250);
const HAPPY_EYEBALLS_SUBSEQUENT_DELAY: Duration = Duration::from_millis(100);

// SNI 伪装连接器
pub struct SniSpoofConnector {
    dns_resolver: Arc<DnsResolver>,
    insecure_config: Arc<rustls::ClientConfig>,
    connect_timeout: Duration,
}

impl SniSpoofConnector {
    pub fn new(dns_resolver: Arc<DnsResolver>, connect_timeout_secs: u64) -> Self {
        let insecure_config = Arc::new(Self::build_insecure_config());

        Self {
            dns_resolver,
            insecure_config,
            connect_timeout: Duration::from_secs(connect_timeout_secs),
        }
    }

    // 跳过证书验证的 TLS 配置
    fn build_insecure_config() -> rustls::ClientConfig {
        let mut config = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoCertVerifier))
            .with_no_client_auth();

        config.alpn_protocols = vec![b"http/1.1".to_vec()];

        config
    }

    // SNI 伪装连接
    pub async fn connect(
        &self,
        real_domain: &str,
        fake_sni: &str,
    ) -> anyhow::Result<TlsStream<TcpStream>> {
        debug!("SNI 伪装: 连接 {} (SNI={})", real_domain, fake_sni);

        let ip = self.dns_resolver.resolve_best(real_domain).await?;
        let addr = std::net::SocketAddr::new(ip, 443);

        debug!("SNI 伪装: 解析 {} -> {}", real_domain, addr);

        let tcp_stream =
            match tokio::time::timeout(self.connect_timeout, TcpStream::connect(addr)).await {
                Ok(Ok(stream)) => {
                    let _ = stream.set_nodelay(true);
                    stream
                }
                Ok(Err(e)) => {
                    anyhow::bail!("TCP 连接失败 {} ({}): {}", real_domain, ip, e);
                }
                Err(_) => {
                    anyhow::bail!(
                        "TCP 连接超时 {} ({}, {}s)",
                        real_domain,
                        ip,
                        self.connect_timeout.as_secs()
                    );
                }
            };

        let connector = tokio_rustls::TlsConnector::from(self.insecure_config.clone());
        let tls_stream = tokio::time::timeout(
            self.connect_timeout,
            connector.connect(
                rustls::pki_types::ServerName::try_from(fake_sni.to_string())?,
                tcp_stream,
            ),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "TLS 握手超时 (SNI={}, {}s)",
                fake_sni,
                self.connect_timeout.as_secs()
            )
        })??;

        debug!(
            "SNI 伪装连接成功: {} (IP: {}, SNI={})",
            real_domain, ip, fake_sni
        );

        Ok(tls_stream)
    }

    // SNI 连接，支持多 IP 竞速 + 国际 DoH 回退 (RFC 8305 Happy Eyeballs v2)
    pub async fn connect_with_fallback(
        &self,
        real_domain: &str,
        fake_sni: &str,
    ) -> anyhow::Result<TlsStream<TcpStream>> {
        let candidates = self.dns_resolver.get_candidates(real_domain).await;

        if candidates.is_empty() {
            anyhow::bail!("没有可用的候选 IP: {}", real_domain);
        }

        match self
            .run_sni_happy_eyeballs(real_domain, fake_sni, &candidates)
            .await
        {
            Ok(stream) => return Ok(stream),
            Err(first_err) => {
                debug!(
                    "国内 IP 全部失败 for {} (SNI={}), 尝试国际 DoH: {}",
                    real_domain, fake_sni, first_err
                );
            }
        }

        self.dns_resolver.invalidate(real_domain);
        let international_ips = self.dns_resolver.resolve_international(real_domain).await?;
        if international_ips.is_empty() {
            anyhow::bail!("国际 DoH 也未返回 IP: {}", real_domain);
        }

        info!(
            "国际回退: {} 获得 {} 个国际 IP 重试 SNI 伪装",
            real_domain,
            international_ips.len()
        );
        self.run_sni_happy_eyeballs(real_domain, fake_sni, &international_ips)
            .await
    }

    async fn run_sni_happy_eyeballs(
        &self,
        real_domain: &str,
        fake_sni: &str,
        candidates: &[std::net::IpAddr],
    ) -> anyhow::Result<TlsStream<TcpStream>> {
        debug!(
            "SNI 伪装: Happy Eyeballs {} (SNI={}, {} 个候选)",
            real_domain,
            fake_sni,
            candidates.len()
        );

        if candidates.len() == 1 {
            return self
                .try_sni_connect(real_domain, fake_sni, candidates[0])
                .await;
        }

        let real_domain = Arc::new(real_domain.to_string());
        let fake_sni = Arc::new(fake_sni.to_string());
        let insecure_config = self.insecure_config.clone();
        let connect_timeout = self.connect_timeout;

        type PendingFuture =
            Pin<Box<dyn std::future::Future<Output = anyhow::Result<TlsStream<TcpStream>>> + Send>>;
        let mut pending: FuturesUnordered<PendingFuture> = FuturesUnordered::new();
        let mut next_idx: usize = 0;

        let rd = real_domain.clone();
        let fs = fake_sni.clone();
        let cfg = insecure_config.clone();
        let ip = candidates[next_idx];
        pending.push(Box::pin(async move {
            Self::do_sni_connect(&rd, &fs, &cfg, ip, connect_timeout).await
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
                            debug!("SNI 伪装: 连接成功 (剩余 {} 个未完成)", pending.len());
                            return Ok(stream);
                        }
                        Err(e) => {
                            debug!("SNI 伪装: 候选连接失败: {}", e);
                            if !more_candidates && pending.is_empty() {
                                anyhow::bail!(
                                    "SNI 伪装: 所有 {} 个候选 IP 均连接失败: {} (SNI={})",
                                    candidates.len(), real_domain, fake_sni
                                );
                            }
                        }
                    }
                }

                _ = &mut delay_timer, if more_candidates => {
                    let rd = real_domain.clone();
                    let fs = fake_sni.clone();
                    let cfg = insecure_config.clone();
                    let ip = candidates[next_idx];
                    pending.push(Box::pin(async move {
                        Self::do_sni_connect(&rd, &fs, &cfg, ip, connect_timeout).await
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

    // SNI 伪装连接（静态方法，用于 'static 上下文）
    async fn do_sni_connect(
        real_domain: &str,
        fake_sni: &str,
        insecure_config: &Arc<rustls::ClientConfig>,
        ip: std::net::IpAddr,
        connect_timeout: Duration,
    ) -> anyhow::Result<TlsStream<TcpStream>> {
        let addr = std::net::SocketAddr::new(ip, 443);
        debug!("SNI 伪装: 尝试连接 {} ({})", real_domain, addr);

        let tcp_stream = tokio::time::timeout(connect_timeout, TcpStream::connect(addr))
            .await
            .map_err(|_| {
                anyhow::anyhow!("TCP 连接超时 ({}, {}s)", ip, connect_timeout.as_secs())
            })??;

        let _ = tcp_stream.set_nodelay(true);

        let connector = tokio_rustls::TlsConnector::from(insecure_config.clone());
        let tls_stream = tokio::time::timeout(
            connect_timeout,
            connector.connect(
                rustls::pki_types::ServerName::try_from(fake_sni.to_string())?,
                tcp_stream,
            ),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "TLS 握手超时 (SNI={}, {}s)",
                fake_sni,
                connect_timeout.as_secs()
            )
        })??;

        debug!(
            "SNI 伪装连接成功: {} (IP: {}, SNI={})",
            real_domain, ip, fake_sni
        );

        Ok(tls_stream)
    }

    // 单 IP 连接
    async fn try_sni_connect(
        &self,
        real_domain: &str,
        fake_sni: &str,
        ip: std::net::IpAddr,
    ) -> anyhow::Result<TlsStream<TcpStream>> {
        let addr = std::net::SocketAddr::new(ip, 443);
        debug!("SNI 伪装: 尝试连接 {} ({})", real_domain, addr);

        let tcp_stream = tokio::time::timeout(self.connect_timeout, TcpStream::connect(addr))
            .await
            .map_err(|_| {
                anyhow::anyhow!("TCP 连接超时 ({}, {}s)", ip, self.connect_timeout.as_secs())
            })??;

        let _ = tcp_stream.set_nodelay(true);

        let connector = tokio_rustls::TlsConnector::from(self.insecure_config.clone());
        let tls_stream = tokio::time::timeout(
            self.connect_timeout,
            connector.connect(
                rustls::pki_types::ServerName::try_from(fake_sni.to_string())?,
                tcp_stream,
            ),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "TLS 握手超时 (SNI={}, {}s)",
                fake_sni,
                self.connect_timeout.as_secs()
            )
        })??;

        debug!(
            "SNI 伪装连接成功: {} (IP: {}, SNI={})",
            real_domain, ip, fake_sni
        );

        Ok(tls_stream)
    }

    // 直接连指定 IP，跳过 DNS
    pub async fn connect_to_ip(
        &self,
        ip: std::net::IpAddr,
        fake_sni: &str,
        port: u16,
    ) -> anyhow::Result<TlsStream<TcpStream>> {
        let addr = std::net::SocketAddr::new(ip, port);

        debug!("SNI 伪装: 直接连接 {} (SNI={})", addr, fake_sni);

        let tcp_stream = TcpStream::connect(addr).await?;
        tcp_stream.set_nodelay(true)?;

        let connector = tokio_rustls::TlsConnector::from(self.insecure_config.clone());
        let tls_stream = connector
            .connect(
                rustls::pki_types::ServerName::try_from(fake_sni.to_string())?,
                tcp_stream,
            )
            .await?;

        debug!("SNI 伪装连接成功: {} (SNI={})", addr, fake_sni);

        Ok(tls_stream)
    }
}

// 跳过证书验证
#[derive(Debug)]
struct NoCertVerifier;

impl rustls::client::danger::ServerCertVerifier for NoCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
        ]
    }
}
