// Windows 系统代理设置

use std::path::PathBuf;
use std::sync::Mutex;

use tracing::info;
use winreg::enums::*;
use winreg::RegKey;

const INTERNET_SETTINGS: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

fn open_settings_key() -> anyhow::Result<RegKey> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    hkcu.open_subkey_with_flags(INTERNET_SETTINGS, KEY_SET_VALUE)
        .map_err(Into::into)
}

fn refresh_proxy() {
    #[link(name = "wininet")]
    extern "system" {
        fn InternetSetOptionW(
            hwnd: isize,
            dwoption: u32,
            lpbuffers: isize,
            dwbufferlength: u32,
        ) -> i32;
    }

    const INTERNET_OPTION_SETTINGS_CHANGED: u32 = 39;
    const INTERNET_OPTION_REFRESH: u32 = 37;

    unsafe {
        let r1 = InternetSetOptionW(0, INTERNET_OPTION_SETTINGS_CHANGED, 0, 0);
        let r2 = InternetSetOptionW(0, INTERNET_OPTION_REFRESH, 0, 0);
        if r1 == 0 || r2 == 0 {
            tracing::warn!("InternetSetOptionW 失败: {}, {}", r1, r2);
        }
    }
}

fn write_pac_config(port: u16) -> anyhow::Result<()> {
    let key = open_settings_key()?;
    key.set_value("ProxyEnable", &0u32)?;
    key.set_value("ProxyServer", &"")?;
    key.set_value("ProxyOverride", &"")?;
    let pac_url = format!("http://127.0.0.1:{}/pac", port);
    key.set_value("AutoConfigURL", &pac_url)?;
    Ok(())
}

fn write_saved_state(key: &RegKey, saved: &SavedProxyState) -> anyhow::Result<()> {
    key.set_value("ProxyEnable", &saved.proxy_enable)?;

    match saved.proxy_server.as_deref() {
        Some(s) if !s.is_empty() => key.set_value("ProxyServer", &s)?,
        _ => key.set_value("ProxyServer", &"")?,
    }

    match saved.proxy_override.as_deref() {
        Some(s) if !s.is_empty() => key.set_value("ProxyOverride", &s)?,
        _ => key.set_value("ProxyOverride", &"")?,
    }

    match saved.auto_config_url.as_deref() {
        Some(s) if !s.is_empty() => key.set_value("AutoConfigURL", &s)?,
        _ => key.set_value("AutoConfigURL", &"")?,
    }

    Ok(())
}

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

#[derive(Debug, Clone, Default)]
struct SavedProxyState {
    proxy_enable: u32,
    proxy_server: Option<String>,
    proxy_override: Option<String>,
    auto_config_url: Option<String>,
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
    state: Mutex<Option<SavedProxyState>>,
    set_by_us: Mutex<bool>,
}

impl ProxyGuard {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(None),
            set_by_us: Mutex::new(false),
        }
    }

    fn read_current_state() -> anyhow::Result<SavedProxyState> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let key = hkcu.open_subkey(INTERNET_SETTINGS)?;

        Ok(SavedProxyState {
            proxy_enable: key.get_value("ProxyEnable").unwrap_or(0),
            proxy_server: key.get_value("ProxyServer").ok(),
            proxy_override: key.get_value("ProxyOverride").ok(),
            auto_config_url: key.get_value("AutoConfigURL").ok(),
        })
    }

    pub async fn set_proxy(&self, port: u16) -> anyhow::Result<()> {
        {
            let mut state = self.state.lock().unwrap();
            if state.is_none() {
                *state = Some(Self::read_current_state()?);
            }
            if let Some(ref saved) = *state {
                let settings = SystemProxySettings {
                    proxy_enabled: saved.proxy_enable != 0,
                    proxy_server: saved.proxy_server.clone(),
                    proxy_override: saved.proxy_override.clone(),
                };
                save_backup(&settings)?;
            }
        }

        write_pac_config(port)?;
        refresh_proxy();
        info!("系统代理已设置 (PAC): http://127.0.0.1:{}/pac", port);

        *self.set_by_us.lock().unwrap() = true;

        Ok(())
    }

    pub async fn restore(&self) -> anyhow::Result<()> {
        self.restore_sync()
    }

    /// 同步版本的恢复方法，用于 shutdown 等需要同步执行的场景
    pub fn restore_sync(&self) -> anyhow::Result<()> {
        let mut state = self.state.lock().unwrap();
        let mut was_set = self.set_by_us.lock().unwrap();

        if !*was_set {
            return Ok(());
        }

        let saved = state.take();
        drop(state);

        if let Some(ref saved) = saved {
            let key = open_settings_key()?;
            write_saved_state(&key, saved)?;
            refresh_proxy();
            delete_backup();

            info!(
                "系统代理已恢复: ProxyEnable={}, ProxyServer={:?}",
                saved.proxy_enable, saved.proxy_server
            );
        }

        *was_set = false;
        Ok(())
    }
}

impl Drop for ProxyGuard {
    fn drop(&mut self) {
        let was_set = self
            .set_by_us
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        if !*was_set {
            return;
        }

        let saved = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();

        if let Some(ref saved) = saved {
            if let Ok(key) = open_settings_key() {
                let _ = write_saved_state(&key, saved);
                refresh_proxy();
                delete_backup();
                info!("系统代理已恢复 (drop)");
            }
        }
    }
}

pub fn restore_proxy_settings() -> anyhow::Result<()> {
    let path = backup_path();
    if !path.exists() {
        return Ok(());
    }

    let content = std::fs::read_to_string(&path)?;
    let settings: SystemProxySettings = serde_json::from_str(&content)?;
    let key = open_settings_key()?;

    let enable_val: u32 = if settings.proxy_enabled { 1 } else { 0 };
    let _ = key.set_value("ProxyEnable", &enable_val);

    match settings.proxy_server.as_deref() {
        Some(server) if !server.is_empty() => {
            let _ = key.set_value("ProxyServer", &server);
        }
        _ => {
            let _ = key.set_value("ProxyServer", &"");
        }
    }

    match settings.proxy_override.as_deref() {
        Some(ov) if !ov.is_empty() => {
            let _ = key.set_value("ProxyOverride", &ov);
        }
        _ => {
            let _ = key.set_value("ProxyOverride", &"");
        }
    }

    let _ = key.set_value("AutoConfigURL", &"");

    refresh_proxy();

    let _ = std::fs::remove_file(&path);
    info!("已从备份恢复系统代理设置");
    Ok(())
}