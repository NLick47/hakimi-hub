use std::sync::Arc;

use rustls::server::ResolvesServerCert;
use rustls::sign::CertifiedKey;
use tokio::net::TcpStream;
use tokio_rustls::server::TlsStream;
use tracing::debug;

use crate::mitm::cert_cache::CertCache;

// 根据 SNI 动态选证书，复用同一个 ServerConfig
struct DynamicCertResolver {
    cert_cache: Arc<CertCache>,
}

impl std::fmt::Debug for DynamicCertResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DynamicCertResolver")
            .field("cert_cache", &"CertCache")
            .finish()
    }
}

impl ResolvesServerCert for DynamicCertResolver {
    fn resolve(&self, client_hello: rustls::server::ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let domain = client_hello.server_name()?;

        debug!("动态证书解析: 请求域名 {}", domain);

        // 在同步上下文里跑 async，用 block_in_place 告诉 tokio 这不是真阻塞
        let cert_pair = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.cert_cache.get_or_generate(domain))
        });

        match cert_pair {
            Ok(pair) => pair.certified_key().ok(),
            Err(e) => {
                debug!("动态证书解析失败 {}: {}", domain, e);
                None
            }
        }
    }
}

// 客户端侧 TLS 终结
pub struct TlsTerminator {
    cert_cache: Arc<CertCache>,
    // 复用的 ServerConfig，动态证书解析器支持多域名
    server_config: Arc<rustls::ServerConfig>,
}

impl TlsTerminator {
    pub fn new(cert_cache: Arc<CertCache>) -> Self {
        let resolver = Arc::new(DynamicCertResolver {
            cert_cache: cert_cache.clone(),
        });

        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(resolver);

        Self {
            cert_cache,
            server_config: Arc::new(config),
        }
    }

    // 握手，确保证书已生成
    pub async fn accept(
        &self,
        client: TcpStream,
        domain: &str,
    ) -> anyhow::Result<TlsStream<TcpStream>> {
        debug!("TLS 终止器: 正在接受 {} 的连接", domain);

        // 先把证书准备好，这样 DynamicCertResolver 能命中缓存
        self.cert_cache.get_or_generate(domain).await?;

        let acceptor = tokio_rustls::TlsAcceptor::from(self.server_config.clone());
        let tls_stream = acceptor.accept(client).await?;

        debug!("与客户端完成 {} 的 TLS 握手", domain);

        Ok(tls_stream)
    }
}
