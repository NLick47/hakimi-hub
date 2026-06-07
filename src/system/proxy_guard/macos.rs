// macOS 系统代理设置
// 使用 networksetup 命令操作

use std::path::PathBuf;
use std::sync::Mutex;

use tracing::{info, warn};

fn backup_path() -> PathBuf {
    crate::utils::paths::data_dir().join("proxy-settings.bak")
}

fn create_backup_marker() {
    // 创建一个空的标记文件，用于崩溃恢复时检测
    let _ = std::fs::write(backup_path(), "");
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

/// 系统代理设置结构体（用于 read_proxy_settings）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SystemProxySettings {
    pub proxy_enabled: bool,
    pub proxy_server: Option<String>,
    pub proxy_override: Option<String>,
}

#[derive(Debug)]
#[must_use = "ProxyGuard 离开作用域时会恢复代理设置"]
pub struct ProxyGuard {
    set_by_us: Mutex<bool>,
}

impl Default for ProxyGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl ProxyGuard {
    pub fn new() -> Self {
        Self {
            set_by_us: Mutex::new(false),
        }
    }

    pub async fn set_proxy(&self, port: u16) -> anyhow::Result<()> {
        // 创建备份标记文件（用于崩溃恢复）
        create_backup_marker();

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
        let mut was_set = self.set_by_us.lock().unwrap_or_else(|e| e.into_inner());

        if !*was_set {
            return Ok(());
        }

        let services = get_network_services();

        // macOS 还原逻辑：只需关闭 PAC 即可
        // 系统会自动回退到之前的代理设置（如果有的话）
        for service in &services {
            let _ = std::process::Command::new("networksetup")
                .args(["-setautoproxyurl", service, ""])
                .output();
        }

        delete_backup();
        info!("系统代理已恢复 (PAC已关闭)");

        *was_set = false;
        Ok(())
    }
}

impl Drop for ProxyGuard {
    fn drop(&mut self) {
        let was_set = self.set_by_us.lock().unwrap_or_else(|e| e.into_inner());

        if !*was_set {
            return;
        }

        let services = get_network_services();

        // macOS 还原逻辑：只需关闭 PAC 即可
        for service in &services {
            let _ = std::process::Command::new("networksetup")
                .args(["-setautoproxyurl", service, ""])
                .output();
        }

        delete_backup();
        info!("系统代理已恢复 (PAC已关闭)");
    }
}

/// 从崩溃中恢复代理设置
/// 检测备份标记文件是否存在，如果存在则关闭 PAC
pub fn restore_proxy_settings() -> anyhow::Result<()> {
    let path = backup_path();
    if !path.exists() {
        return Ok(());
    }

    let services = get_network_services();

    // macOS 还原逻辑：只需关闭 PAC 即可
    for service in &services {
        let _ = std::process::Command::new("networksetup")
            .args(["-setautoproxyurl", service, ""])
            .output();
    }

    let _ = std::fs::remove_file(&path);
    info!("已从崩溃恢复中还原系统代理设置 (PAC已关闭)");
    Ok(())
}
