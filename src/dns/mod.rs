pub mod doh_client;
pub mod ip_prober;
pub mod resolver;
pub mod stream_resolver;
pub mod working_ip_store;

pub use ip_prober::IpProber;
pub use stream_resolver::{StreamResolveResult, StreamResolver};
pub use working_ip_store::WorkingIpStore;

use std::net::IpAddr;

// IP 过滤级别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpFilterLevel {
    // 不过滤，全部保留
    None,
    // 严格模式：过滤无效 IP（0.0.0.0、广播地址等），保留私有 IP
    Strict,
    // 公网模式：过滤私有 IP + 无效 IP，防 DNS 污染
    PublicOnly,
}

// 判断 IP 是否无效（必须过滤）
// 包括：未指定地址、回环、链路本地、广播、文档用途、组播、保留地址
pub fn is_invalid_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_unspecified()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || is_reserved_ipv4(v4)
        }
        IpAddr::V6(v6) => {
            v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_multicast()
                // Link-local IPv6: fe80::/10
                || matches!(v6.segments()[0], 0xfe80..=0xfebf)
        }
    }
}

// IPv4 保留地址判断 (240.0.0.0/4, E 类)
// 排除 255.255.255.255（广播地址）
fn is_reserved_ipv4(v4: &std::net::Ipv4Addr) -> bool {
    let octets = v4.octets();
    octets[0] >= 240 && octets[0] < 255
}

// 判断是否私有/内网 IP
// IPv4: 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
// IPv6: fc00::/7 (唯一本地地址)
pub fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private(),
        IpAddr::V6(v6) => {
            // Unique local addresses: fc00::/7
            matches!(v6.segments()[0], 0xfc00..=0xfdff)
        }
    }
}

// 已废弃，改用 filter_ips + IpFilterLevel
#[deprecated(note = "Use filter_ips with IpFilterLevel for more control")]
pub fn is_suspicious(ip: &IpAddr) -> bool {
    is_invalid_ip(ip) || is_private_ip(ip)
}

// 按级别过滤 IP
// 全部被过滤时返回原始列表（防止断网）
pub fn filter_ips(ips: &[IpAddr], level: IpFilterLevel) -> Vec<IpAddr> {
    if level == IpFilterLevel::None {
        return ips.to_vec();
    }

    let filtered: Vec<IpAddr> = ips
        .iter()
        .filter(|ip| {
            if is_invalid_ip(ip) {
                return false; // Always filter invalid IPs
            }
            if level == IpFilterLevel::PublicOnly && is_private_ip(ip) {
                return false; // Filter private IPs in public-only mode
            }
            true
        })
        .cloned()
        .collect();

    // Fallback protection: if all IPs are filtered, return original list
    // This prevents breaking connectivity when the filter is too aggressive
    if filtered.is_empty() && !ips.is_empty() {
        tracing::warn!("IP 过滤后结果为空，回退到原始列表（{} 个 IP）", ips.len());
        return ips.to_vec();
    }

    filtered
}

// 已废弃，改用 filter_ips + IpFilterLevel
#[deprecated(note = "Use filter_ips with IpFilterLevel for more control")]
pub fn filter_suspicious(ips: &[IpAddr]) -> Vec<IpAddr> {
    filter_ips(ips, IpFilterLevel::PublicOnly)
}
