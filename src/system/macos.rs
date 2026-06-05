use anyhow::Result;

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
                .skip(1)
                .filter(|l| !l.is_empty() && !l.starts_with('*'))
                .map(|s| s.to_string())
                .collect()
        }
        Err(_) => vec!["Wi-Fi".to_string()],
    }
}

// 从 macOS 读取当前代理设置
pub fn read_proxy_settings() -> Result<crate::system::proxy_guard::SystemProxySettings> {
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

    Ok(crate::system::proxy_guard::SystemProxySettings {
        proxy_enabled: enabled,
        proxy_server,
        proxy_override: None,
    })
}
