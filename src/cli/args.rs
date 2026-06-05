use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "hakimi-hub", version, about = "GitHub 访问代理工具")]
pub struct AppArgs {
    #[command(subcommand)]
    pub command: Command,

    #[arg(long, global = true, env = "HAKIMI_HUB_CONFIG")]
    pub config: Option<String>,

    #[arg(long, global = true, default_value = if cfg!(debug_assertions) { "debug" } else { "warn" })]
    pub log_level: String,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    Start {
        /// 代理端口（覆盖配置文件）
        #[arg(long)]
        port: Option<u16>,

        /// 禁用 MITM 拦截
        #[arg(long)]
        no_mitm: bool,

        /// 禁用 UI，只显示日志
        #[arg(long)]
        no_ui: bool,
    },

    /// 停止运行中的代理服务器
    Stop,

    /// 显示运行状态
    Status,

    /// 导出 CA 证书
    ExportCa {
        /// 输出文件路径
        #[arg(long, short)]
        output: Option<String>,
    },

    /// 配置 Git 使用代理
    GitSetup,

    /// 恢复 Git 配置
    GitTeardown,

    /// 崩溃后手动恢复
    Recover,

    /// 配置管理
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// 切换或显示主题
    Theme {
        /// 主题名称: pink / morning / noon / night / auto
        /// 不指定则显示当前主题和可用主题列表
        name: Option<String>,
    },

    /// 缓存管理
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum CacheAction {
    /// 清理所有缓存
    Clear,

    /// 清理过期缓存
    ClearExpired,

    /// 显示缓存统计信息
    Stats,
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// 显示当前配置
    Show,

    /// 生成默认配置文件
    Init,
}
