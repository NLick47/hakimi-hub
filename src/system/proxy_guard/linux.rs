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
        info!(
            "Linux 下请手动配置系统代理，PAC 地址: http://127.0.0.1:{}/pac",
            port
        );
        *self.set_by_us.lock().unwrap() = true;
        Ok(())
    }

    pub async fn restore(&self) -> anyhow::Result<()> {
        self.restore_sync()
    }

    pub fn restore_sync(&self) -> anyhow::Result<()> {
        *self.set_by_us.lock().unwrap() = false;
        Ok(())
    }
}

impl Drop for ProxyGuard {
    fn drop(&mut self) {}
}

pub fn restore_proxy_settings() -> anyhow::Result<()> {
    Ok(())
}
