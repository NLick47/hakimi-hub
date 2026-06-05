use serde::{Deserialize, Serialize};

use crate::utils;

// ─── Default value functions ────────────────────────────────────────

fn default_true() -> bool {
    true
}
fn default_false() -> bool {
    false
}

fn default_proxy_bind() -> String {
    "127.0.0.1".to_string()
}
fn default_proxy_port() -> u16 {
    2830
}
fn default_proxy_max_connections() -> usize {
    0
}
fn default_proxy_idle_timeout() -> u64 {
    300
}

fn default_banner_theme() -> String {
    "pink".to_string()
}

fn default_dns_probe_timeout() -> u64 {
    8
}
fn default_dns_max_concurrent() -> usize {
    32
}
fn default_dns_cache_ttl() -> u64 {
    300
}
fn default_dns_probe_interval() -> u64 {
    300
}
fn default_dns_sni_spoof() -> String {
    "www.baidu.com".to_string()
}
fn default_dns_connect_timeout() -> u64 {
    10
}
fn default_max_cache_entries() -> Option<usize> {
    Some(1000)
}
fn default_ipv6_failure_threshold() -> u32 {
    3
}
fn default_ipv6_recovery_secs() -> u64 {
    300
}
fn default_ipv6_enabled() -> bool {
    false
} // 默认禁用 IPv6

fn default_cert_key_size() -> usize {
    4096
}
fn default_cert_validity() -> u32 {
    7
}
fn default_cert_max_cache() -> usize {
    1000
}

fn default_rules_update_interval() -> u64 {
    24
}

fn default_log_level() -> String {
    "info".to_string()
}
fn default_log_file() -> String {
    "logs/hakimi-hub.log".to_string()
}
fn default_log_retention() -> u64 {
    7
}

fn default_cache_ttl_days() -> u64 {
    30
}
fn default_cache_max_size_mb() -> usize {
    500
}
fn default_cache_max_file_size_mb() -> usize {
    10
}

// ─── Banner Config ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BannerConfig {
    /// 主题模式: "pink" (默认粉色系), "morning" (金色晨光), "noon" (清凉薄荷), "night" (紫夜), "auto" (根据时间自动切换)
    #[serde(default = "default_banner_theme")]
    pub theme: String,
}

impl Default for BannerConfig {
    fn default() -> Self {
        Self {
            theme: default_banner_theme(),
        }
    }
}

// ─── AppConfig ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub dns: DnsConfig,
    #[serde(default)]
    pub cert: CertConfig,
    #[serde(default)]
    pub rules: RulesConfig,
    #[serde(default)]
    pub log: LogConfig,
    #[serde(default)]
    pub banner: BannerConfig,
    #[serde(default)]
    pub intercepts: InterceptsConfig,
    #[serde(default)]
    pub cache: CacheConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            proxy: ProxyConfig::default(),
            dns: DnsConfig::default(),
            cert: CertConfig::default(),
            rules: RulesConfig::default(),
            log: LogConfig::default(),
            banner: BannerConfig::default(),
            intercepts: InterceptsConfig::default(),
            cache: CacheConfig::default(),
        }
    }
}

impl AppConfig {
    /// Load configuration from a file
    pub fn load_from_file(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: AppConfig = toml::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    /// Load from default config file, or use built-in defaults if not found
    pub fn load_or_default() -> anyhow::Result<Self> {
        let config_path = utils::paths::data_dir().join("config.toml");
        if config_path.exists() {
            Self::load_from_file(&config_path.to_string_lossy())
        } else {
            // 配置文件不存在，直接使用代码默认值（不写文件，避免固化默认值）
            Ok(Self::default())
        }
    }

    /// Validate configuration
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.proxy.port == 0 {
            anyhow::bail!("proxy.port cannot be 0");
        }
        if self.proxy.idle_timeout_secs < 10 {
            anyhow::bail!("proxy.idle_timeout_secs must be >= 10");
        }
        if self.dns.probe_timeout_secs == 0 || self.dns.probe_timeout_secs > 60 {
            anyhow::bail!("dns.probe_timeout_secs must be 1-60");
        }
        if self.dns.cache_ttl_secs < 30 {
            anyhow::bail!("dns.cache_ttl_secs must be >= 30");
        }
        if self.dns.max_concurrent_probes == 0 {
            anyhow::bail!("dns.max_concurrent_probes cannot be 0");
        }
        if self.cert.ca_key_size != 2048 && self.cert.ca_key_size != 4096 {
            anyhow::bail!("cert.ca_key_size must be 2048 or 4096");
        }
        if self.cert.cert_validity_days == 0 {
            anyhow::bail!("cert.cert_validity_days cannot be 0");
        }
        Ok(())
    }

    /// Generate a minimal config template with comments for user customization
    pub fn generate_template() -> String {
        r##"# Hakimi Hub 用户配置
# 未列出的配置使用代码内置默认值
# 取消注释并修改即可自定义

[proxy]
# port = 2830                   # 代理端口
# bind = "127.0.0.1"            # 监听地址
# mitm_enabled = true           # 是否启用 MITM 拦截
# auto_set_system_proxy = true  # 启动时自动设置系统代理

[banner]
# theme = "pink"                # pink / morning / noon / night / auto

# [[intercepts.rules]]
# 镜像站代理示例:
# from = "github.com"
# from_pattern = "^/[^/]+/[^/]+/releases/download/.*"
# action = { Proxy = { mirror = "v4.gh-proxy.org" } }
#
# SNI 伪装示例:
# from = "github.com"
# action = { SniSpoof = { sni = "www.baidu.com", real_host = "github.com" } }
#
# 拦截示例:
# from = "translate.google.com"
# action = { Abort = true }
"##
        .to_string()
    }
}

// ─── Intercept Config ─────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterceptsConfig {
    #[serde(default)]
    pub rules: Vec<InterceptRule>,
}

impl Default for InterceptsConfig {
    fn default() -> Self {
        InterceptsConfig::common_defaults()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterceptRule {
    pub from: String,
    #[serde(default)]
    pub from_pattern: Option<String>,
    pub action: InterceptAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InterceptAction {
    /// 镜像站代理（单镜像）
    Proxy { mirror: String },
    /// SNI 伪装
    SniSpoof { sni: String, real_host: String },
    /// HTTP 重定向
    Redirect { target: String },
    /// 直接关闭连接
    Abort(bool),
}

impl InterceptsConfig {
    pub fn common_defaults() -> Self {
        Self {
            rules: vec![
                // ─── GitHub 加速规则 ─────────────────────────────────────
                InterceptRule {
                    from: "github.com".into(),
                    from_pattern: Some(r"^/[^/]+/[^/]+/releases/download/.*".into()),
                    action: InterceptAction::Proxy {
                        mirror: "v4.gh-proxy.org".into(),
                    },
                },
                InterceptRule {
                    from: "github.com".into(),
                    from_pattern: Some(r"^/[^/]+/[^/]+/archive/.*".into()),
                    action: InterceptAction::Proxy {
                        mirror: "v4.gh-proxy.org".into(),
                    },
                },
                InterceptRule {
                    from: "codeload.github.com".into(),
                    from_pattern: Some(r"^/.*".into()),
                    action: InterceptAction::Proxy {
                        mirror: "v4.gh-proxy.org".into(),
                    },
                },
                InterceptRule {
                    from: "github.com".into(),
                    from_pattern: None,
                    action: InterceptAction::SniSpoof {
                        sni: "www.baidu.com".into(),
                        real_host: "github.com".into(),
                    },
                },
                InterceptRule {
                    from: "api.github.com".into(),
                    from_pattern: None,
                    action: InterceptAction::SniSpoof {
                        sni: "www.baidu.com".into(),
                        real_host: "api.github.com".into(),
                    },
                },
                InterceptRule {
                    from: "*.githubassets.com".into(),
                    from_pattern: None,
                    action: InterceptAction::SniSpoof {
                        sni: "www.baidu.com".into(),
                        real_host: "*.githubassets.com".into(),
                    },
                },
                InterceptRule {
                    from: "release-assets.githubusercontent.com".into(),
                    from_pattern: None,
                    action: InterceptAction::SniSpoof {
                        sni: "www.baidu.com".into(),
                        real_host: "release-assets.githubusercontent.com".into(),
                    },
                },
                InterceptRule {
                    from: "objects.githubusercontent.com".into(),
                    from_pattern: None,
                    action: InterceptAction::SniSpoof {
                        sni: "www.baidu.com".into(),
                        real_host: "objects.githubusercontent.com".into(),
                    },
                },
                InterceptRule {
                    from: "*.githubusercontent.com".into(),
                    from_pattern: None,
                    action: InterceptAction::SniSpoof {
                        sni: "www.baidu.com".into(),
                        real_host: "*.githubusercontent.com".into(),
                    },
                },
                // ─── Steam 加速规则 ───────────────────────────────────────
                InterceptRule {
                    from: "store.steampowered.com".into(),
                    from_pattern: None,
                    action: InterceptAction::SniSpoof {
                        sni: "www.baidu.com".into(),
                        real_host: "store.steampowered.com".into(),
                    },
                },
                InterceptRule {
                    from: "api.steampowered.com".into(),
                    from_pattern: None,
                    action: InterceptAction::SniSpoof {
                        sni: "www.baidu.com".into(),
                        real_host: "api.steampowered.com".into(),
                    },
                },
                InterceptRule {
                    from: "login.steampowered.com".into(),
                    from_pattern: None,
                    action: InterceptAction::SniSpoof {
                        sni: "www.baidu.com".into(),
                        real_host: "login.steampowered.com".into(),
                    },
                },
                InterceptRule {
                    from: "help.steampowered.com".into(),
                    from_pattern: None,
                    action: InterceptAction::SniSpoof {
                        sni: "www.baidu.com".into(),
                        real_host: "help.steampowered.com".into(),
                    },
                },
                InterceptRule {
                    from: "store-points.steampowered.com".into(),
                    from_pattern: None,
                    action: InterceptAction::SniSpoof {
                        sni: "www.baidu.com".into(),
                        real_host: "store-points.steampowered.com".into(),
                    },
                },
                InterceptRule {
                    from: "*.steamstatic.com".into(),
                    from_pattern: None,
                    action: InterceptAction::SniSpoof {
                        sni: "www.baidu.com".into(),
                        real_host: "*.steamstatic.com".into(),
                    },
                },
                InterceptRule {
                    from: "steamserver.net".into(),
                    from_pattern: None,
                    action: InterceptAction::SniSpoof {
                        sni: "www.baidu.com".into(),
                        real_host: "steamserver.net".into(),
                    },
                },
                InterceptRule {
                    from: "support.steampowered.com".into(),
                    from_pattern: None,
                    action: InterceptAction::SniSpoof {
                        sni: "www.baidu.com".into(),
                        real_host: "support.steampowered.com".into(),
                    },
                },
            ],
        }
    }
}

// ─── Proxy Config ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    /// 代理监听地址
    #[serde(default = "default_proxy_bind")]
    pub bind: String,
    /// 代理监听端口
    #[serde(default = "default_proxy_port")]
    pub port: u16,
    /// 是否启用 MITM 拦截
    #[serde(default = "default_true")]
    pub mitm_enabled: bool,
    /// 最大并发连接数（0 = 无限制）
    #[serde(default = "default_proxy_max_connections")]
    pub max_connections: usize,
    /// 连接空闲超时（秒）
    #[serde(default = "default_proxy_idle_timeout")]
    pub idle_timeout_secs: u64,
    /// 启动时是否自动设置系统代理
    #[serde(default = "default_true")]
    pub auto_set_system_proxy: bool,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            bind: default_proxy_bind(),
            port: default_proxy_port(),
            mitm_enabled: default_true(),
            max_connections: default_proxy_max_connections(),
            idle_timeout_secs: default_proxy_idle_timeout(),
            auto_set_system_proxy: default_true(),
        }
    }
}

// ─── DNS 配置 ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsConfig {
    /// DoH 后端供应商列表
    #[serde(default = "default_doh_endpoints")]
    pub doh_endpoints: Vec<DohEndpoint>,
    /// TCP 探测超时（秒）
    #[serde(default = "default_dns_probe_timeout")]
    pub probe_timeout_secs: u64,
    /// 最大并发探测数
    #[serde(default = "default_dns_max_concurrent")]
    pub max_concurrent_probes: usize,
    /// 缓存 TTL（秒）
    #[serde(default = "default_dns_cache_ttl")]
    pub cache_ttl_secs: u64,
    /// IP 测速间隔（秒），定时重新测速选择最佳 IP
    #[serde(default = "default_dns_probe_interval")]
    pub probe_interval_secs: u64,
    /// SNI 伪装域名（用于绕过系统代理分流规则）
    #[serde(default = "default_dns_sni_spoof")]
    pub sni_spoof_domain: String,
    /// 单个 IP 连接超时（秒），超时后自动尝试下一个候选 IP
    #[serde(default = "default_dns_connect_timeout")]
    pub connect_timeout_secs: u64,
    /// DNS 缓存最大条目数（防止内存无限增长）
    #[serde(default = "default_max_cache_entries")]
    pub max_cache_entries: Option<usize>,
    /// IPv6 连续探测失败多少次后暂时禁用 IPv6（0 表示不禁用）
    #[serde(default = "default_ipv6_failure_threshold")]
    pub ipv6_failure_threshold: u32,
    /// IPv6 禁用后多久重新尝试探测（秒）
    #[serde(default = "default_ipv6_recovery_secs")]
    pub ipv6_recovery_secs: u64,
    /// 是否启用 IPv6 探测（默认禁用，多数网络环境 IPv6 不可用或延迟高）
    #[serde(default = "default_ipv6_enabled")]
    pub ipv6_enabled: bool,
    /// 域名 -> DoH 提供商映射
    /// 例如: { "*.github.com": "alidns", "github.com": "tencent" }
    /// 用于指定特定域名使用特定的 DoH 提供商
    #[serde(default)]
    pub dns_mapping: std::collections::HashMap<String, String>,
}

fn default_doh_endpoints() -> Vec<DohEndpoint> {
    vec![
        DohEndpoint {
            name: "alidns".into(),
            url: "https://dns.alidns.com/dns-query".into(),
            priority: 10,
            trusted: false,
            preset_ip: Some("223.5.5.5".into()),
        },
        DohEndpoint {
            name: "alidns-backup".into(),
            url: "https://dns.alidns.com/dns-query".into(),
            priority: 11,
            trusted: false,
            preset_ip: Some("223.6.6.6".into()),
        },
        DohEndpoint {
            name: "tencent-doh".into(),
            url: "https://doh.pub/dns-query".into(),
            priority: 12,
            trusted: false,
            preset_ip: Some("119.29.29.29".into()),
        },
        DohEndpoint {
            name: "dnspod".into(),
            url: "https://doh.pub/dns-query".into(),
            priority: 13,
            trusted: false,
            preset_ip: Some("119.29.29.29".into()),
        },
        DohEndpoint {
            name: "360-doh".into(),
            url: "https://doh.360.cn/dns-query".into(),
            priority: 14,
            trusted: false,
            preset_ip: Some("101.226.4.6".into()),
        },
        DohEndpoint {
            name: "baidu-doh".into(),
            url: "https://doh.baidu.com/dns-query".into(),
            priority: 15,
            trusted: false,
            preset_ip: Some("180.76.76.76".into()),
        },
        DohEndpoint {
            name: "cloudflare-1".into(),
            url: "https://cloudflare-dns.com/dns-query".into(),
            priority: 30,
            trusted: true,
            preset_ip: Some("104.16.249.249".into()),
        },
        DohEndpoint {
            name: "cloudflare-2".into(),
            url: "https://cloudflare-dns.com/dns-query".into(),
            priority: 31,
            trusted: true,
            preset_ip: Some("104.16.248.249".into()),
        },
        DohEndpoint {
            name: "cloudflare-3".into(),
            url: "https://cloudflare-dns.com/dns-query".into(),
            priority: 32,
            trusted: true,
            preset_ip: Some("104.16.132.229".into()),
        },
        DohEndpoint {
            name: "cloudflare-4".into(),
            url: "https://cloudflare-dns.com/dns-query".into(),
            priority: 33,
            trusted: true,
            preset_ip: Some("162.159.36.1".into()),
        },
        DohEndpoint {
            name: "cloudflare-5".into(),
            url: "https://cloudflare-dns.com/dns-query".into(),
            priority: 34,
            trusted: true,
            preset_ip: Some("162.159.46.1".into()),
        },
        DohEndpoint {
            name: "google-dns-1".into(),
            url: "https://dns.google/dns-query".into(),
            priority: 40,
            trusted: true,
            preset_ip: Some("8.8.8.8".into()),
        },
        DohEndpoint {
            name: "google-dns-2".into(),
            url: "https://dns.google/dns-query".into(),
            priority: 41,
            trusted: true,
            preset_ip: Some("8.8.4.4".into()),
        },
        DohEndpoint {
            name: "quad9".into(),
            url: "https://dns.quad9.net/dns-query".into(),
            priority: 50,
            trusted: true,
            preset_ip: Some("9.9.9.9".into()),
        },
        DohEndpoint {
            name: "adguard".into(),
            url: "https://dns.adguard-dns.com/dns-query".into(),
            priority: 60,
            trusted: true,
            preset_ip: Some("94.140.14.14".into()),
        },
        // IPv6 端点已移除（默认禁用 IPv6，这些端点不会用到）
        // 如需 IPv6，可在配置文件中手动添加
    ]
}

impl Default for DnsConfig {
    fn default() -> Self {
        Self {
            doh_endpoints: default_doh_endpoints(),
            probe_timeout_secs: default_dns_probe_timeout(),
            max_concurrent_probes: default_dns_max_concurrent(),
            cache_ttl_secs: default_dns_cache_ttl(),
            probe_interval_secs: default_dns_probe_interval(),
            sni_spoof_domain: default_dns_sni_spoof(),
            connect_timeout_secs: default_dns_connect_timeout(),
            max_cache_entries: default_max_cache_entries(),
            ipv6_failure_threshold: default_ipv6_failure_threshold(),
            ipv6_recovery_secs: default_ipv6_recovery_secs(),
            ipv6_enabled: default_ipv6_enabled(),
            dns_mapping: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DohEndpoint {
    pub name: String,
    pub url: String,
    pub priority: u8,
    pub trusted: bool,
    #[serde(default)]
    pub preset_ip: Option<String>,
}

// ─── Certificate Config ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertConfig {
    /// CA key size (2048 or 4096)
    #[serde(default = "default_cert_key_size")]
    pub ca_key_size: usize,
    /// Domain certificate validity in days
    #[serde(default = "default_cert_validity")]
    pub cert_validity_days: u32,
    /// Maximum cached certificates
    #[serde(default = "default_cert_max_cache")]
    pub max_cache_size: usize,
}

impl Default for CertConfig {
    fn default() -> Self {
        Self {
            ca_key_size: default_cert_key_size(),
            cert_validity_days: default_cert_validity(),
            max_cache_size: default_cert_max_cache(),
        }
    }
}

// ─── Rules Config ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RulesConfig {
    /// Rule update interval in hours
    #[serde(default = "default_rules_update_interval")]
    pub update_interval_hours: u64,
    /// Rule sources
    #[serde(default = "default_rules_sources")]
    pub sources: Vec<RuleSource>,
}

fn default_rules_sources() -> Vec<RuleSource> {
    vec![RuleSource {
        name: "builtin".into(),
        source_type: "builtin".into(),
        url: String::new(),
    }]
}

impl Default for RulesConfig {
    fn default() -> Self {
        Self {
            update_interval_hours: default_rules_update_interval(),
            sources: default_rules_sources(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleSource {
    pub name: String,
    pub source_type: String,
    #[serde(default)]
    pub url: String,
}

// ─── Log Config ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogConfig {
    /// Log level
    #[serde(default = "default_log_level")]
    pub level: String,
    /// Log file path (relative to data dir)
    #[serde(default = "default_log_file")]
    pub file: String,
    /// Log retention in days
    #[serde(default = "default_log_retention")]
    pub retention_days: u64,
    /// Enable JSON log format
    #[serde(default = "default_false")]
    pub json_format: bool,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            file: default_log_file(),
            retention_days: default_log_retention(),
            json_format: default_false(),
        }
    }
}

// ─── Cache Config ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    /// 是否启用静态资源缓存
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 缓存有效期（天）
    #[serde(default = "default_cache_ttl_days")]
    pub ttl_days: u64,
    /// 最大缓存大小（MB），超过此大小清理最旧缓存
    #[serde(default = "default_cache_max_size_mb")]
    pub max_size_mb: usize,
    /// 最大单个文件大小（MB），超过此大小不缓存
    #[serde(default = "default_cache_max_file_size_mb")]
    pub max_file_size_mb: usize,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            ttl_days: default_cache_ttl_days(),
            max_size_mb: default_cache_max_size_mb(),
            max_file_size_mb: default_cache_max_file_size_mb(),
        }
    }
}
