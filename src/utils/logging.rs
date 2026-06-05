use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::utils::paths;

// 自定义时间格式：只显示时分秒
struct LocalTimeHMS;

impl tracing_subscriber::fmt::time::FormatTime for LocalTimeHMS {
    fn format_time(&self, w: &mut tracing_subscriber::fmt::format::Writer<'_>) -> std::fmt::Result {
        use time::OffsetDateTime;
        let now = OffsetDateTime::now_local().map_err(|_| std::fmt::Error)?;
        write!(
            w,
            "{:02}:{:02}:{:02}",
            now.hour(),
            now.minute(),
            now.second()
        )
    }
}

pub fn init(level: &str, retention_days: usize, enable_console: bool) -> anyhow::Result<()> {
    paths::ensure_all_dirs()?;

    let file_appender = tracing_appender::rolling::RollingFileAppender::builder()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("hakimi-hub")
        .filename_suffix("log")
        .max_log_files(retention_days)
        .build(paths::log_dir())?;

    // 文件日志（始终启用，使用 trace 级别）
    let file_layer = tracing_subscriber::fmt::layer()
        .with_timer(LocalTimeHMS)
        .with_writer(file_appender)
        .with_ansi(false)
        .with_target(true);

    // 控制台日志（可选）
    let console_layer = tracing_subscriber::fmt::layer()
        .with_timer(LocalTimeHMS)
        .with_target(false)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false);

    // 全局 filter：控制台使用用户指定级别，文件使用 trace
    // 由于 per-layer filter 会导致类型问题，这里用全局 filter + 文件层单独 filter
    let console_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(level))
        .add_directive("rustls=warn".parse().expect("rustls directive is valid"));

    if enable_console {
        tracing_subscriber::registry()
            .with(console_filter)
            .with(file_layer)
            .with(console_layer)
            .try_init()?;
    } else {
        // 仅文件日志，用 trace 级别
        let file_filter = EnvFilter::new("trace")
            .add_directive("rustls=warn".parse().expect("rustls directive is valid"));
        tracing_subscriber::registry()
            .with(file_filter)
            .with(file_layer)
            .try_init()?;
    }

    Ok(())
}
