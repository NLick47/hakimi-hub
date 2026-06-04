use anyhow::Result;

pub fn read_proxy_settings() -> Result<crate::system::proxy_guard::SystemProxySettings> {
    let output = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.system.proxy", "mode"])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            let mode = String::from_utf8_lossy(&output.stdout);
            let enabled = mode.contains("'manual'") || mode.contains("'auto'");

            let proxy_server = if enabled {
                let host_output = std::process::Command::new("gsettings")
                    .args(["get", "org.gnome.system.proxy.http", "host"])
                    .output()
                    .ok();
                let port_output = std::process::Command::new("gsettings")
                    .args(["get", "org.gnome.system.proxy.http", "port"])
                    .output()
                    .ok();

                match (host_output, port_output) {
                    (Some(h), Some(p)) if h.status.success() && p.status.success() => {
                        let host = String::from_utf8_lossy(&h.stdout)
                            .trim()
                            .trim_matches('\'')
                            .to_string();
                        let port = String::from_utf8_lossy(&p.stdout).trim().to_string();

                        if !host.is_empty() && host != "''" {
                            Some(format!("{}:{}", host, port))
                        } else {
                            None
                        }
                    }
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
        _ => {
            let proxy_server = std::env::var("http_proxy")
                .or_else(|_| std::env::var("HTTP_PROXY"))
                .ok();

            Ok(crate::system::proxy_guard::SystemProxySettings {
                proxy_enabled: proxy_server.is_some(),
                proxy_server,
                proxy_override: None,
            })
        }
    }
}