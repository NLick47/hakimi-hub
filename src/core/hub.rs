use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::cache::ResourceCache;
use crate::core::config::AppConfig;
use crate::core::shutdown::{CrashRecovery, ShutdownSignal};
use crate::dns::resolver::DnsResolver;
use crate::intercepts::matcher::InterceptMatcher;
use crate::intercepts::mirror_health::MirrorHealthTracker;
use crate::mitm::ca::CertificateAuthority;
use crate::mitm::cert_cache::CertCache;
use crate::mitm::tls_origin::TlsOrigin;
use crate::mitm::tls_terminator::TlsTerminator;
use crate::proxy::handler::HandlerContext;
use crate::proxy::metrics::Metrics;
use crate::proxy::server::ProxyServer;
use crate::rules::builtin::DomainRules;
use crate::system::proxy_guard::ProxyGuard;
use crate::utils;

pub struct HakimiHub {
    config: Arc<AppConfig>,
    running: Arc<AtomicBool>,
    shutdown_tx: broadcast::Sender<()>,
    proxy_guard: Option<ProxyGuard>,
}

impl HakimiHub {
    pub fn new(config: AppConfig) -> Self {
        let (shutdown_tx, _) = broadcast::channel(16);

        Self {
            config: Arc::new(config),
            running: Arc::new(AtomicBool::new(false)),
            shutdown_tx,
            proxy_guard: None,
        }
    }

    pub async fn start(&mut self, no_ui: bool) -> anyhow::Result<()> {
        utils::process::acquire_single_instance()?;

        let recovery = CrashRecovery::new();
        if recovery.check_crash_marker() {
            warn!("上次会话未正常关闭，正在执行恢复...");
            recovery.recover_all()?;
        }
        recovery.create_crash_marker()?;

        self.running.store(true, Ordering::SeqCst);

        let config = &*self.config;
        let data_dir = utils::paths::data_dir();
        let theme = &config.banner.theme;

        // 创建启动 UI（如果启用）
        let startup_ui = if !no_ui {
            utils::banner::print_banner_with_theme(theme);
            Some(utils::ui::StartupUI::new(theme))
        } else {
            None
        };

        // 步骤 1: DNS
        if let Some(ref ui) = startup_ui {
            ui.print_header();
            ui.step_dns();
        }
        debug!("正在初始化 DNS 解析器...");
        let dns_resolver = Arc::new(DnsResolver::new(&config.dns)?);

        // 步骤 2: CA 证书
        debug!("正在初始化证书颁发机构...");
        let ca_path = data_dir.join("ca.crt.pem");
        let ca_exists = ca_path.exists();
        if let Some(ref ui) = startup_ui {
            ui.step_ca(ca_exists);
        }

        let ca = Arc::new(
            CertificateAuthority::load_or_generate(&data_dir, config.cert.ca_key_size).await?,
        );
        let cert_cache = Arc::new(CertCache::new(
            ca.clone(),
            utils::paths::cert_cache_dir(),
            config.cert.max_cache_size,
            config.cert.cert_validity_days,
        ));

        let tls_terminator = Arc::new(TlsTerminator::new(cert_cache.clone()));
        let tls_origin = Arc::new(TlsOrigin::new(
            dns_resolver.clone(),
            String::new(),
            String::new(),
            config.dns.connect_timeout_secs,
        ));

        let domain_rules = Arc::new(DomainRules::github_defaults());
        let metrics = Metrics::new();
        let intercept_matcher = Arc::new(InterceptMatcher::new(config.intercepts.rules.clone()));
        let mirror_health = Arc::new(MirrorHealthTracker::default_tracker());

        // 初始化资源缓存
        let resource_cache = Arc::new(ResourceCache::new(
            utils::paths::cache_dir(),
            config.cache.clone(),
        ));

        let handler_ctx = Arc::new(HandlerContext::new(
            config.proxy.mitm_enabled,
            ca.clone(),
            cert_cache.clone(),
            tls_terminator.clone(),
            tls_origin.clone(),
            domain_rules.clone(),
            intercept_matcher.clone(),
            mirror_health.clone(),
            metrics.clone(),
            resource_cache.clone(),
            config.dns.sni_spoof_domain.clone(),
            config.proxy.mitm_enabled,
            config.proxy.port,
            config.proxy.idle_timeout_secs,
        ));

        // 步骤 3: 系统代理
        let proxy_port = config.proxy.port;
        if config.proxy.auto_set_system_proxy {
            if let Some(ref ui) = startup_ui {
                ui.step_proxy();
            }
            info!("正在设置系统代理...");
            let guard = ProxyGuard::new();
            guard.set_proxy(proxy_port).await?;
            self.proxy_guard = Some(guard);
        }

        // 步骤 4: 启动服务
        if let Some(ref ui) = startup_ui {
            ui.step_server();
        }

        let proxy_server = ProxyServer::new(config.proxy.clone(), metrics.clone(), handler_ctx);
        let shutdown_rx = self.shutdown_tx.subscribe();

        // 完成启动 UI
        if let Some(ref ui) = startup_ui {
            ui.finish(
                &config.proxy.bind,
                config.proxy.port,
                config.proxy.mitm_enabled,
            );
        }

        // 运行面板（只在启用 UI 时）
        let panel_handle = if !no_ui {
            let runtime_panel = Arc::new(utils::ui::RuntimePanel::new(
                metrics.clone(),
                mirror_health,
                theme,
            ));

            runtime_panel.print_panel();

            let panel_interval = Duration::from_secs(1);
            let mut shutdown_rx_for_panel = self.shutdown_tx.subscribe();
            Some(tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(panel_interval) => {
                            runtime_panel.print_panel();
                        }
                        _ = shutdown_rx_for_panel.recv() => {
                            break;
                        }
                    }
                }
            }))
        } else {
            None
        };

        let proxy_handle = tokio::spawn(async move {
            if let Err(e) = proxy_server.start(shutdown_rx).await {
                tracing::error!("代理服务器错误: {}", e);
            }
        });

        let probe_interval = dns_resolver.probe_interval();
        let mut shutdown_rx_for_probe = self.shutdown_tx.subscribe();
        let probe_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(probe_interval) => {
                        dns_resolver.reprobe_all().await;
                    }
                    _ = shutdown_rx_for_probe.recv() => {
                        debug!("IP 测速任务收到关闭信号，正在退出...");
                        break;
                    }
                }
            }
        });

        let shutdown = ShutdownSignal::new(self.shutdown_tx.clone());

        shutdown.wait().await;

        proxy_handle.abort();

        tokio::time::timeout(Duration::from_secs(2), probe_handle)
            .await
            .ok();

        if let Some(handle) = panel_handle {
            tokio::time::timeout(Duration::from_secs(1), handle)
                .await
                .ok();
        }

        self.stop(no_ui).await
    }

    pub async fn stop(&mut self, no_ui: bool) -> anyhow::Result<()> {
        if !self.running.swap(false, Ordering::SeqCst) {
            return Ok(());
        }

        info!("正在关闭 Hakimi Hub...");

        if let Some(guard) = self.proxy_guard.take() {
            guard.restore_sync()?;
            info!("系统代理已恢复");
        }

        // 移除崩溃标记（正常关闭）
        let recovery = CrashRecovery::new();
        recovery.remove_crash_marker()?;

        // 清理 PID 文件
        utils::process::release_single_instance();

        // 打印道别消息（只在启用 UI 时）
        if !no_ui {
            let theme = &self.config.banner.theme;
            utils::ui::goodbye(theme);
        }

        info!("Hakimi Hub 已停止");
        Ok(())
    }

    pub fn status(&self) -> HubStatus {
        HubStatus {
            running: self.running.load(Ordering::SeqCst),
        }
    }

    pub fn shutdown_rx(&self) -> broadcast::Receiver<()> {
        self.shutdown_tx.subscribe()
    }

    pub fn trigger_shutdown(&self) {
        let _ = self.shutdown_tx.send(());
    }

    pub fn config(&self) -> Arc<AppConfig> {
        self.config.clone()
    }
}

#[derive(Debug, Clone)]
pub struct HubStatus {
    pub running: bool,
}

impl Drop for HakimiHub {
    fn drop(&mut self) {
        if self.running.swap(false, Ordering::SeqCst) {
            if let Some(guard) = self.proxy_guard.take() {
                warn!("进程异常退出，正在恢复系统代理...");
                if let Err(e) = guard.restore_sync() {
                    warn!("恢复系统代理失败: {}", e);
                }
            }

            let recovery = CrashRecovery::new();
            recovery.remove_crash_marker().ok();
            utils::process::release_single_instance();
        }
    }
}
