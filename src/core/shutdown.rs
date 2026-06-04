use std::path::PathBuf;

use tracing::{info, warn};

use crate::utils;

// 崩溃恢复管理器
pub struct CrashRecovery {
    marker_file: PathBuf,
}

impl CrashRecovery {
    pub fn new() -> Self {
        let data_dir = utils::paths::data_dir();
        Self {
            marker_file: data_dir.join("crash-marker"),
        }
    }

    // 检查崩溃标记文件是否存在（表示上次未正常关闭）
    pub fn check_crash_marker(&self) -> bool {
        self.marker_file.exists()
    }

    // 创建崩溃标记文件（启动时调用）
    pub fn create_crash_marker(&self) -> std::io::Result<()> {
        std::fs::write(&self.marker_file, std::process::id().to_string())
    }

    // 删除崩溃标记文件（正常关闭时调用）
    pub fn remove_crash_marker(&self) -> std::io::Result<()> {
        if self.marker_file.exists() {
            std::fs::remove_file(&self.marker_file)
        } else {
            Ok(())
        }
    }

    // 执行所有恢复操作
    pub fn recover_all(&self) -> anyhow::Result<()> {
        info!("Running crash recovery...");

        // 恢复系统代理设置
        if let Err(e) = self.restore_system_proxy() {
            warn!("Failed to restore system proxy: {}", e);
        }

        // 恢复 Git 配置
        if let Err(e) = self.restore_git_config() {
            warn!("Failed to restore Git config: {}", e);
        }

        // 清理临时文件
        self.cleanup_temp_files();

        // 删除崩溃标记
        self.remove_crash_marker()?;

        info!("Crash recovery completed");
        Ok(())
    }

    fn restore_system_proxy(&self) -> anyhow::Result<()> {
        crate::system::proxy_guard::restore_proxy_settings()
    }

    fn restore_git_config(&self) -> anyhow::Result<()> {
        crate::git::config::teardown()
    }

    fn cleanup_temp_files(&self) {
        let data_dir = utils::paths::data_dir();
        let temp_marker = data_dir.join("proxy-settings.bak");
        if temp_marker.exists() {
            if let Err(e) = std::fs::remove_file(&temp_marker) {
                warn!("Failed to remove temp file {}: {}", temp_marker.display(), e);
            }
        }
    }
}

impl Default for CrashRecovery {
    fn default() -> Self {
        Self::new()
    }
}

// 关闭信号处理器
pub struct ShutdownSignal {
    tx: tokio::sync::broadcast::Sender<()>,
}

impl ShutdownSignal {
    pub fn new(tx: tokio::sync::broadcast::Sender<()>) -> Self {
        Self { tx }
    }

    // 等待关闭信号（Ctrl+C、SIGTERM 或手动触发）
    pub async fn wait(&self) {
        let mut rx = self.tx.subscribe();

        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};

            // 注册 SIGINT 处理器（覆盖默认行为）
            let mut sigint = signal(SignalKind::interrupt())
                .expect("Failed to setup SIGINT handler");
            let mut sigterm = signal(SignalKind::terminate())
                .expect("Failed to setup SIGTERM handler");

            tokio::select! {
                // Ctrl+C (SIGINT)
                _ = sigint.recv() => {
                    info!("Received SIGINT (Ctrl+C), shutting down...");
                }
                // SIGTERM (kill 命令默认发送)
                _ = sigterm.recv() => {
                    info!("Received SIGTERM, shutting down...");
                }
                // 广播关闭
                _ = rx.recv() => {
                    info!("Received shutdown signal");
                }
            }
        }

        #[cfg(not(unix))]
        {
            tokio::select! {
                // Ctrl+C
                _ = tokio::signal::ctrl_c() => {
                    info!("Received Ctrl+C, shutting down...");
                }
                // 广播关闭
                _ = rx.recv() => {
                    info!("Received shutdown signal");
                }
            }
        }
    }
}

