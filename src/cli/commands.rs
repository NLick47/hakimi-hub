use anyhow::Result;
use tracing::info;

use crate::cache::ResourceCache;
use crate::cli::args::{AppArgs, Command, ConfigAction, CacheAction};
use crate::core::config::AppConfig;
use crate::core::hub::HakimiHub;
use crate::utils;

pub async fn run(args: AppArgs) -> Result<()> {

    let config = if let Some(path) = &args.config {
        AppConfig::load_from_file(path)?
    } else {
        AppConfig::load_or_default()?
    };

    let theme = &config.banner.theme;

    match args.command {
        Command::Start {
            port,
            no_mitm,
            no_ui,
        } => {
            utils::logging::init(&args.log_level, config.log.retention_days as usize, no_ui)?;

            let mut config = config;

            if let Some(p) = port {
                config.proxy.port = p;
            }
            if no_mitm {
                config.proxy.mitm_enabled = false;
            }

            let mut hub = HakimiHub::new(config);

            hub.start(no_ui).await?;
        }

        Command::Stop => {
            // 其他命令：始终输出日志到控制台
            utils::logging::init(&args.log_level, config.log.retention_days as usize, true)?;

            let ui = utils::ui::CommandOutput::new(theme);
            let pid = utils::process::read_pid_file()?;
            match pid {
                Some(pid) => {
                    #[cfg(unix)]
                    {
                        let _ = std::process::Command::new("kill")
                            .arg(pid.to_string())
                            .output();
                    }
                    #[cfg(windows)]
                    {
                        let result = std::process::Command::new("taskkill")
                            .args(["/PID", &pid.to_string()])
                            .output();
                        if let Ok(output) = result {
                            if !output.status.success() {
                                tracing::warn!("优雅关闭失败，尝试强制终止...");
                                let _ = std::process::Command::new("taskkill")
                                    .args(["/PID", &pid.to_string(), "/F"])
                                    .output();
                            }
                        }
                    }
                    ui.stopped(Some(pid));
                }
                None => {
                    ui.stopped(None);
                }
            }
        }

        Command::Status => {
            utils::logging::init(&args.log_level, config.log.retention_days as usize, true)?;
            let ui = utils::ui::CommandOutput::new(theme);
            match utils::process::read_pid_file()? {
                Some(pid) => {
                    let running = utils::process::is_process_alive_pub(pid);
                    if running {
                        ui.status(true, Some(pid));
                    } else {
                        ui.status(false, None);
                        let _ = std::fs::remove_file(crate::utils::paths::pid_file_path());
                    }
                }
                None => {
                    ui.status(false, None);
                }
            }
        }

        Command::ExportCa { output } => {
            utils::logging::init(&args.log_level, config.log.retention_days as usize, true)?;
            let ui = utils::ui::CommandOutput::new(theme);
            let _config = if let Some(path) = &args.config {
                AppConfig::load_from_file(path)?
            } else {
                AppConfig::load_or_default()?
            };

            let data_dir = utils::paths::data_dir();
            let ca_cert_path = data_dir.join("ca.crt.pem");

            if !ca_cert_path.exists() {
                ui.error(&format!(
                    "CA 证书未找到: {}。请先启动服务以生成证书。",
                    ca_cert_path.display()
                ));
                anyhow::bail!(
                    "CA 证书未找到: {}。请先启动服务以生成证书。",
                    ca_cert_path.display()
                );
            }

            let cert_data = std::fs::read_to_string(&ca_cert_path)?;
            match output {
                Some(path) => {
                    std::fs::write(&path, &cert_data)?;
                    ui.ca_exported(&path);
                }
                None => {
                    info!("CA 证书路径: {}", ca_cert_path.display());
                    println!();
                    println!("{}", cert_data);
                }
            }
        }

        Command::GitSetup => {
            utils::logging::init(&args.log_level, config.log.retention_days as usize, true)?;
            let ui = utils::ui::CommandOutput::new(theme);
            let config = if let Some(path) = &args.config {
                AppConfig::load_from_file(path)?
            } else {
                AppConfig::load_or_default()?
            };
            crate::git::config::setup(&config)?;
            ui.git_setup();
        }

        Command::GitTeardown => {
            utils::logging::init(&args.log_level, config.log.retention_days as usize, true)?;
            let ui = utils::ui::CommandOutput::new(theme);
            crate::git::config::teardown()?;
            ui.git_teardown();
        }

        Command::Recover => {
            utils::logging::init(&args.log_level, config.log.retention_days as usize, true)?;
            let ui = utils::ui::CommandOutput::new(theme);
            info!("正在执行崩溃恢复...");
            let recovery = crate::core::shutdown::CrashRecovery::new();
            recovery.recover_all()?;
            ui.success("恢复完成");
        }

        Command::Config { action } => {
            utils::logging::init(&args.log_level, config.log.retention_days as usize, true)?;
            let ui = utils::ui::CommandOutput::new(theme);
            match action {
                ConfigAction::Show => {
                    let config = if let Some(path) = &args.config {
                        AppConfig::load_from_file(path)?
                    } else {
                        AppConfig::load_or_default()?
                    };
                    let toml = toml::to_string_pretty(&config)?;
                    println!("{}", toml);
                }
                ConfigAction::Init => {
                    let path = args.config.unwrap_or_else(|| {
                        utils::paths::data_dir()
                            .join("config.toml")
                            .to_string_lossy()
                            .to_string()
                    });

                    if let Some(parent) = std::path::Path::new(&path).parent() {
                        std::fs::create_dir_all(parent)?;
                    }

                    let template = AppConfig::generate_template();
                    std::fs::write(&path, &template)?;
                    ui.config_init(&path);
                }
            }
        }

        Command::Theme { name } => {
            utils::logging::init(&args.log_level, config.log.retention_days as usize, false)?;
            handle_theme_command(name, &args.config)?;
        }

        Command::Cache { action } => {
            utils::logging::init(&args.log_level, config.log.retention_days as usize, true)?;
            handle_cache_command(action, &config).await?;
        }
    }

    Ok(())
}

fn handle_theme_command(name: Option<String>, config_path: &Option<String>) -> Result<()> {
    let themes = [
        ("pink", "pink", "粉色系"),
        ("morning", "morning", "金色晨光"),
        ("noon", "noon", "薄荷清凉 薄荷？薄荷！"),
        ("night", "night", "深夜蓝"),
        ("auto", "auto", "根据时间自动选择"),
    ];

    match name {
        Some(theme_name) => {
            if !themes.iter().any(|(key, _, _)| *key == theme_name) {
                eprintln!("未知主题: {}", theme_name);
                eprintln!();
                eprintln!("可用主题:");
                for (key, name, desc) in &themes {
                    eprintln!("  {:10} {} - {}", key, name, desc);
                }
                anyhow::bail!("无效的主题名称");
            }

            let config_path = config_path.clone().unwrap_or_else(|| {
                utils::paths::data_dir()
                    .join("config.toml")
                    .to_string_lossy()
                    .to_string()
            });

            let mut config = if std::path::Path::new(&config_path).exists() {
                AppConfig::load_from_file(&config_path)?
            } else {
                AppConfig::default()
            };

            config.banner.theme = theme_name.clone();
            if let Some(parent) = std::path::Path::new(&config_path).parent() {
                std::fs::create_dir_all(parent)?;
            }

            let toml = toml::to_string_pretty(&config)?;
            std::fs::write(&config_path, &toml)?;

            utils::banner::print_banner_with_theme(&theme_name);
            eprintln!();
            eprintln!("  主题已切换为: {}", theme_name);
            eprintln!("  重启服务后生效");
        }

        None => {
            let current = if let Some(path) = config_path {
                AppConfig::load_from_file(path)
                    .map(|c| c.banner.theme)
                    .unwrap_or_else(|_| "pink".to_string())
            } else {
                AppConfig::load_or_default()
                    .map(|c| c.banner.theme)
                    .unwrap_or_else(|_| "pink".to_string())
            };

            eprintln!();
            eprintln!("  当前主题: {}", current);
            eprintln!();
            eprintln!("  可用主题:");
            for (key, _, desc) in &themes {
                let marker = if *key == current { " *" } else { "" };
                eprintln!("    {:10} {}{}", key, desc, marker);
            }
            eprintln!();
            eprintln!("  使用方法: hakimi-hub theme <主题名>");
            eprintln!("  示例: hakimi-hub theme night");
        }
    }

    Ok(())
}

async fn handle_cache_command(action: CacheAction, config: &AppConfig) -> Result<()> {
    let cache_dir = utils::paths::cache_dir();
    let resource_cache = ResourceCache::new(cache_dir.clone(), config.cache.clone());

    match action {
        CacheAction::Clear => {
            let count = resource_cache.clear().await?;
            eprintln!();
            eprintln!("  已清理 {} 个缓存文件", count);
            eprintln!("  缓存目录: {}", cache_dir.display());
        }

        CacheAction::ClearExpired => {
            let count = resource_cache.clear_expired().await?;
            eprintln!();
            if count > 0 {
                eprintln!("  已清理 {} 个过期缓存", count);
            } else {
                eprintln!("  没有过期缓存");
            }
            eprintln!("  缓存目录: {}", cache_dir.display());
        }

        CacheAction::Stats => {
            let stats = resource_cache.stats();
            eprintln!();
            eprintln!("  缓存目录: {}", stats.cache_dir.display());
            eprintln!("  总缓存大小: {:.2} MB", stats.total_size as f64 / 1024.0 / 1024.0);
            eprintln!("  文件数量: {} 个", stats.file_count);

            if let Some(oldest) = stats.oldest {
                let oldest_time = chrono::DateTime::from_timestamp(oldest as i64, 0)
                    .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|| oldest.to_string());
                eprintln!("  最早缓存: {}", oldest_time);
            }

            if let Some(newest) = stats.newest {
                let newest_time = chrono::DateTime::from_timestamp(newest as i64, 0)
                    .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|| newest.to_string());
                eprintln!("  最近缓存: {}", newest_time);
            }

            eprintln!();
            eprintln!("  缓存有效期: {} 天", config.cache.ttl_days);
            eprintln!("  最大缓存大小: {} MB", config.cache.max_size_mb);
            eprintln!("  单文件大小限制: {} MB", config.cache.max_file_size_mb);
        }
    }

    Ok(())
}
