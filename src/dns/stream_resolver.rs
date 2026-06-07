use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::debug;

use crate::core::config::DohEndpoint;
use crate::dns::doh_client::DohClient;

const DOH_QUERY_TIMEOUT: Duration = Duration::from_secs(3);
const DOH_MAX_RETRIES: usize = 3;
const RETRY_DELAY: Duration = Duration::from_millis(300);

pub struct StreamResolveResult {
    pub ip: IpAddr,
    pub source: String,
}

pub struct StreamResolver {
    doh_client: Arc<DohClient>,
    endpoints: Vec<DohEndpoint>,
}

impl StreamResolver {
    pub fn new(doh_client: Arc<DohClient>) -> Self {
        let endpoints = doh_client.endpoints().to_vec();
        Self { doh_client, endpoints }
    }

    pub fn resolve_stream(&self, domain: &str) -> mpsc::Receiver<StreamResolveResult> {
        let (tx, rx) = mpsc::channel(32);
        let domain = domain.to_string();
        let endpoints = self.endpoints.clone();
        let doh_client = self.doh_client.clone();

        tokio::spawn(async move {
            let mut tasks = Vec::new();

            for endpoint in endpoints {
                let tx = tx.clone();
                let domain = domain.clone();
                let doh_client = doh_client.clone();
                let host = extract_host(&endpoint.url);

                if host.is_none() {
                    continue;
                }
                let host = host.unwrap().to_string();

                tasks.push(tokio::spawn(async move {
                    for attempt in 1..=DOH_MAX_RETRIES {
                        match doh_client.query_single_host(&host, &domain, DOH_QUERY_TIMEOUT).await {
                            Ok(ips) if !ips.is_empty() => {
                                debug!("DoH {} 返回 {} 个 IP (attempt {})", host, ips.len(), attempt);
                                for ip in ips {
                                    if matches!(ip, IpAddr::V4(_))
                                        && tx
                                            .send(StreamResolveResult {
                                                ip,
                                                source: host.clone(),
                                            })
                                            .await
                                            .is_err()
                                    {
                                        return;
                                    }
                                }
                                return;
                            }
                            Ok(_) => {
                                debug!("DoH {} 返回空结果 (attempt {}/{})", host, attempt, DOH_MAX_RETRIES);
                            }
                            Err(e) => {
                                debug!("DoH {} 查询失败: {} (attempt {}/{})", host, e, attempt, DOH_MAX_RETRIES);
                            }
                        }

                        if attempt < DOH_MAX_RETRIES {
                            tokio::time::sleep(RETRY_DELAY).await;
                        }
                    }
                }));
            }

            let _ = futures::future::join_all(tasks).await;
        });

        rx
    }
}

fn extract_host(url: &str) -> Option<&str> {
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let host = without_scheme.split('/').next()?;
    host.split(':').next()
}
