// Linux 系统代理设置
// Linux 桌面环境差异大，这里只提供空实现

use std::sync::Mutex;
use tracing::info;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SystemProxySettings {
    pub proxy_enabled: bool,
    pub proxy_server: Option<String>,
    pub proxy_override: Option<String>,
}

#[derive(Debug)]
pub struct ProxyGuard {
    _state: Mutex<Option<SystemProxySettings>>,
}

impl ProxyGuard {
    pub fn new() -> Self {
        Self { _state: Mutex::new(None) }
    }

    pub async fn set_proxy(&self, port: u16) -> anyhow::Result<()> {
        // Linux 下通常需要手动配置，或者使用环境变量
        info!("Linux 下请手动配置系统代理: http://127.0.0.1:{}/pac", port);
        Ok(())
    }

    pub async fn restore(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

impl Drop for ProxyGuard {
    fn drop(&mut self) {}
}

pub fn restore_proxy_settings() -> anyhow::Result<()> {
    Ok(())
}