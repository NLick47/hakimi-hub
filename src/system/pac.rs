/// Generate PAC script content
///
/// 代理 GitHub 和 Steam 相关域名，其余全部直连
pub fn generate_pac(proxy_port: u16) -> String {
    format!(
        r#"// Hakimi Hub PAC - 代理 GitHub 和 Steam 域名
function FindProxyForURL(url, host) {{
    // GitHub 主站 & API
    if (dnsDomainIs(host, "github.com") ||
        dnsDomainIs(host, "api.github.com") ||
        dnsDomainIs(host, "codeload.github.com")) {{
        return "PROXY 127.0.0.1:{port}";
    }}

    // GitHub 资源域名（通配符匹配）
    if (shExpMatch(host, "*.githubassets.com") ||
        shExpMatch(host, "*.githubusercontent.com") ||
        shExpMatch(host, "*.github.dev") ||
        shExpMatch(host, "*.github.io")) {{
        return "PROXY 127.0.0.1:{port}";
    }}

    // Steam 主站 & API
    if (dnsDomainIs(host, "store.steampowered.com") ||
        dnsDomainIs(host, "api.steampowered.com") ||
        dnsDomainIs(host, "login.steampowered.com") ||
        dnsDomainIs(host, "help.steampowered.com") ||
        dnsDomainIs(host, "store-points.steampowered.com") ||
        dnsDomainIs(host, "support.steampowered.com") ||
        dnsDomainIs(host, "steamserver.net")) {{
        return "PROXY 127.0.0.1:{port}";
    }}

    // Steam 资源域名（通配符匹配）
    if (shExpMatch(host, "*.steamstatic.com") ||
        shExpMatch(host, "*.steampowered.com")) {{
        return "PROXY 127.0.0.1:{port}";
    }}

    // 其余全部直连
    return "DIRECT";
}}"#,
        port = proxy_port
    )
}

/// Get the PAC URL for system proxy configuration
pub fn pac_url(proxy_port: u16) -> String {
    format!("http://127.0.0.1:{}/pac", proxy_port)
}
