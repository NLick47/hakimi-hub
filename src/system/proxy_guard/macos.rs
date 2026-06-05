// macOS 系统代理设置
// 使用 networksetup 命令操作

use std::path::PathBuf;
use std::sync::Mutex;

use tracing::{info, warn};

fn backup_path() -> PathBuf {
    crate::utils::paths::data_dir().join("proxy-settings.bak")
}

fn save_backup(settings: &SystemProxySettings) -> anyhow::Result<()> {
    let content = serde_json::to_string(settings)?;
    std::fs::write(backup_path(), content)?;
    Ok(())
}

fn delete_backup() {
    let path = backup_path();
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
}

// 获取所有网络服务
fn get_network_services() -> Vec<String> {
    let output = std::process::Command::new("networksetup")
        .args(["-listallnetworkservices"])
        .output();

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout
                .lines()
                .skip(1) // 第一行是标题
                .filter(|l| !l.is_empty() && !l.starts_with('*'))
                .map(|s| s.to_string())
                .collect()
        }
        Err(e) => {
            warn!("获取网络服务列表失败: {}", e);
            vec!["Wi-Fi".to_string()] // 回退到默认
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SystemProxySettings {
    pub proxy_enabled: bool,
    pub proxy_server: Option<String>,
    pub proxy_override: Option<String>,
}

#[derive(Debug)]
#[must_use = "ProxyGuard 离开作用域时会恢复代理设置"]
pub struct ProxyGuard {
    state: Mutex<Option<SystemProxySettings>>,
    set_by_us: Mutex<bool>,
}

impl ProxyGuard {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(None),
            set_by_us: Mutex::new(false),
        }
    }

    fn read_current_state() -> anyhow::Result<SystemProxySettings> {
        let services = get_network_services();
        let service = services.first().map(|s| s.as_str()).unwrap_or("Wi-Fi");

        let output = std::process::Command::new("networksetup")
            .args(["-getwebproxy", service])
            .output()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let enabled = stdout.contains("Enabled: Yes");

        let proxy_server = if enabled {
            let server = stdout
                .lines()
                .find(|l| l.starts_with("Server:"))
                .and_then(|l| l.split(':').nth(1))
                .map(|s| s.trim().to_string());

            let port = stdout
                .lines()
                .find(|l| l.starts_with("Port:"))
                .and_then(|l| l.split(':').nth(1))
                .map(|s| s.trim().to_string());

            match (server, port) {
                (Some(s), Some(p)) => Some(format!("{}:{}", s, p)),
                _ => None,
            }
        } else {
            None
        };

        Ok(SystemProxySettings {
            proxy_enabled: enabled,
            proxy_server,
            proxy_override: None,
        })
    }

    pub async fn set_proxy(&self, port: u16) -> anyhow::Result<()> {
        {
            let mut state = self.state.lock().unwrap();
            if state.is_none() {
                *state = Some(Self::read_current_state()?);
            }
            if let Some(ref saved) = *state {
                save_backup(saved)?;
            }
        }

        let pac_url = format!("http://127.0.0.1:{}/pac", port);
        let services = get_network_services();

        for service in &services {
            // 关闭手动代理
            let _ = std::process::Command::new("networksetup")
                .args(["-setwebproxystate", service, "off"])
                .output();

            let _ = std::process::Command::new("networksetup")
                .args(["-setsecurewebproxystate", service, "off"])
                .output();

            // 设置 PAC
            let _ = std::process::Command::new("networksetup")
                .args(["-setautoproxyurl", service, &pac_url])
                .output();
        }

        info!("系统代理已设置 (PAC): {}", pac_url);
        *self.set_by_us.lock().unwrap() = true;

        Ok(())
    }

    pub async fn restore(&self) -> anyhow::Result<()> {
        self.restore_sync()
    }

    /// 同步恢复代理设置（用于 Drop 和信号处理）
    pub fn restore_sync(&self) -> anyhow::Result<()> {
        // 使用 unwrap_or_else 处理 poisoned mutex，确保 Drop 时不会 panic
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut was_set = self.set_by_us.lock().unwrap_or_else(|e| e.into_inner());

        if !*was_set {
            return Ok(());
        }

        let saved = state.take();

        if let Some(ref saved) = saved {
            let services = get_network_services();

            for service in &services {
                // 关闭 PAC
                let _ = std::process::Command::new("networksetup")
                    .args(["-setautoproxyurl", service, ""])
                    .output();

                if saved.proxy_enabled {
                    if let Some(ref server_port) = saved.proxy_server {
                        let parts: Vec<&str> = server_port.split(':').collect();
                        if parts.len() == 2 {
                            let server = parts[0];
                            let port = parts[1];

                            let _ = std::process::Command::new("networksetup")
                                .args(["-setwebproxy", service, server, port])
                                .output();

                            let _ = std::process::Command::new("networksetup")
                                .args(["-setsecurewebproxy", service, server, port])
                                .output();

                            let _ = std::process::Command::new("networksetup")
                                .args(["-setwebproxystate", service, "on"])
                                .output();

                            let _ = std::process::Command::new("networksetup")
                                .args(["-setsecurewebproxystate", service, "on"])
                                .output();
                        }
                    }
                } else {
                    let _ = std::process::Command::new("networksetup")
                        .args(["-setwebproxystate", service, "off"])
                        .output();

                    let _ = std::process::Command::new("networksetup")
                        .args(["-setsecurewebproxystate", service, "off"])
                        .output();
                }
            }

            delete_backup();
            info!("系统代理已恢复: enabled={}", saved.proxy_enabled);
        }

        *was_set = false;
        Ok(())
    }
}

impl Drop for ProxyGuard {
    fn drop(&mut self) {
        // 使用与 Windows 一致的逻辑，先检查 was_set 标志
        let was_set = self.set_by_us.lock().unwrap_or_else(|e| e.into_inner());

        if !*was_set {
            return;
        }

        // 获取保存的状态
        let saved = self.state.lock().unwrap_or_else(|e| e.into_inner()).take();

        if let Some(ref saved) = saved {
            let services = get_network_services();

            for service in &services {
                // 关闭 PAC
                let _ = std::process::Command::new("networksetup")
                    .args(["-setautoproxyurl", service, ""])
                    .output();

                if saved.proxy_enabled {
                    if let Some(ref server_port) = saved.proxy_server {
                        let parts: Vec<&str> = server_port.split(':').collect();
                        if parts.len() == 2 {
                            let server = parts[0];
                            let port = parts[1];

                            let _ = std::process::Command::new("networksetup")
                                .args(["-setwebproxy", service, server, port])
                                .output();

                            let _ = std::process::Command::new("networksetup")
                                .args(["-setsecurewebproxy", service, server, port])
                                .output();

                            let _ = std::process::Command::new("networksetup")
                                .args(["-setwebproxystate", service, "on"])
                                .output();

                            let _ = std::process::Command::new("networksetup")
                                .args(["-setsecurewebproxystate", service, "on"])
                                .output();
                        }
                    }
                } else {
                    let _ = std::process::Command::new("networksetup")
                        .args(["-setwebproxystate", service, "off"])
                        .output();

                    let _ = std::process::Command::new("networksetup")
                        .args(["-setsecurewebproxystate", service, "off"])
                        .output();
                }
            }

            delete_backup();
            info!("系统代理已恢复: enabled={}", saved.proxy_enabled);
        }

        // 重置标志
        *self.set_by_us.lock().unwrap_or_else(|e| e.into_inner()) = false;
    }
}

pub fn restore_proxy_settings() -> anyhow::Result<()> {
    let path = backup_path();
    if !path.exists() {
        return Ok(());
    }

    let content = std::fs::read_to_string(&path)?;
    let settings: SystemProxySettings = serde_json::from_str(&content)?;

    let services = get_network_services();

    for service in &services {
        let _ = std::process::Command::new("networksetup")
            .args(["-setautoproxyurl", service, ""])
            .output();

        if settings.proxy_enabled {
            if let Some(ref server_port) = settings.proxy_server {
                let parts: Vec<&str> = server_port.split(':').collect();
                if parts.len() == 2 {
                    let _ = std::process::Command::new("networksetup")
                        .args(["-setwebproxy", service, parts[0], parts[1]])
                        .output();

                    let _ = std::process::Command::new("networksetup")
                        .args(["-setwebproxystate", service, "on"])
                        .output();
                }
            }
        }
    }

    let _ = std::fs::remove_file(&path);
    info!("已从备份恢复系统代理设置");
    Ok(())
}
