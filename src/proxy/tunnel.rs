use std::sync::Arc;
use std::time::Duration;
use tokio::io::copy_bidirectional;
use tokio::net::TcpStream;
use tracing::{debug, warn};

use crate::mitm::tls_origin::TlsOrigin;
use crate::proxy::metrics::Metrics;

const TUNNEL_TIMEOUT: Duration = Duration::from_secs(3600);

pub async fn establish_tunnel(
    mut client: TcpStream,
    host: &str,
    port: u16,
    tls_origin: &Arc<TlsOrigin>,
    use_doh: bool,
    metrics: &Arc<Metrics>,
) -> anyhow::Result<()> {
    debug!("正在建立隧道到 {}:{} (DoH={})", host, port, use_doh);

    let mut target = if use_doh {
        match tls_origin.connect_tcp_with_fallback(host, port).await {
            Ok(stream) => stream,
            Err(e) => {
                warn!("所有 IP 均连接失败 {}:{}: {}", host, port, e);
                return Err(e);
            }
        }
    } else {
        let addr = format!("{}:{}", host, port);
        match TcpStream::connect(&addr).await {
            Ok(stream) => stream,
            Err(e) => {
                warn!("无法连接到 {}: {}", addr, e);
                return Err(e.into());
            }
        }
    };

    target.set_nodelay(true)?;

    match tokio::time::timeout(TUNNEL_TIMEOUT, copy_bidirectional(&mut client, &mut target)).await {
        Ok(Ok((client_to_server, server_to_client))) => {
            metrics.add_bytes_sent(client_to_server);
            metrics.add_bytes_received(server_to_client);
            debug!(
                "隧道关闭 {}, 客户端->服务器 {} 字节, 服务器->客户端 {} 字节",
                host, client_to_server, server_to_client
            );
        }
        Ok(Err(e)) => {
            debug!("隧道双向复制错误 {}: {}", host, e);
        }
        Err(_) => {
            warn!("隧道传输超时 {} ({}s)", host, TUNNEL_TIMEOUT.as_secs());
        }
    }

    Ok(())
}
