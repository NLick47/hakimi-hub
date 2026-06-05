use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tracing::{error, info, warn};

use crate::core::config::ProxyConfig;
use crate::proxy::handler::HandlerContext;
use crate::proxy::metrics::Metrics;

pub struct ProxyContext {
    pub config: ProxyConfig,
    pub metrics: Arc<Metrics>,
    pub handler_ctx: Arc<HandlerContext>,
}

pub struct ProxyServer {
    ctx: ProxyContext,
}

impl ProxyServer {
    pub fn new(
        config: ProxyConfig,
        metrics: Arc<Metrics>,
        handler_ctx: Arc<HandlerContext>,
    ) -> Self {
        Self {
            ctx: ProxyContext {
                config,
                metrics,
                handler_ctx,
            },
        }
    }

    pub fn metrics(&self) -> Arc<Metrics> {
        self.ctx.metrics.clone()
    }

    pub async fn start(
        &self,
        mut shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> anyhow::Result<()> {
        let bind_addr = format!("{}:{}", self.ctx.config.bind, self.ctx.config.port);
        let listener = TcpListener::bind(&bind_addr).await?;

        let max_conn = if self.ctx.config.max_connections > 0 {
            Some(Arc::new(Semaphore::new(self.ctx.config.max_connections)))
        } else {
            None
        };

        loop {
            tokio::select! {
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((stream, addr)) => {
                            let sem = max_conn.clone();
                            let metrics = self.ctx.metrics.clone();
                            let handler_ctx = self.ctx.handler_ctx.clone();

                            tokio::spawn(async move {
                                let _permit = if let Some(s) = &sem {
                                    match s.acquire().await {
                                        Ok(p) => Some(p),
                                        Err(_) => {
                                            warn!("连接数已达上限，拒绝 {}", addr);
                                            return;
                                        }
                                    }
                                } else {
                                    None
                                };

                                metrics.inc_active();

                                if let Err(e) = crate::proxy::handler::handle_connection(stream, handler_ctx).await {
                                    error!("来自 {} 的连接错误: {}", addr, e);
                                }
                                metrics.dec_active();
                            });
                        }
                        Err(e) => {
                            error!("接受连接错误: {}", e);
                        }
                    }
                }
                _ = shutdown.recv() => {
                    info!("代理服务器正在关闭...");
                    break;
                }
            }
        }

        Ok(())
    }
}
