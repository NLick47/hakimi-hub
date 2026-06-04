use anyhow::Result;

// Windows 注册表路径
const REG_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

// 从 Windows 注册表读取当前代理设置（供读取使用）
pub fn read_proxy_settings() -> Result<crate::system::proxy_guard::SystemProxySettings> {
    use winreg::enums::{HKEY_CURRENT_USER};
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu.open_subkey(REG_PATH)?;

    let proxy_enabled: u32 = key.get_value("ProxyEnable").unwrap_or(0);
    let proxy_server: Option<String> = key.get_value("ProxyServer").ok();
    let proxy_override: Option<String> = key.get_value("ProxyOverride").ok();

    Ok(crate::system::proxy_guard::SystemProxySettings {
        proxy_enabled: proxy_enabled != 0,
        proxy_server,
        proxy_override,
    })
}