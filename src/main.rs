use anyhow::Result;
use clap::Parser;
use hakimi_hub::cli;

fn main() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    let args = cli::args::AppArgs::parse();
    let rt = tokio::runtime::Runtime::new()?;

    let result = rt.block_on(async { cli::commands::run(args).await });

    // 给 Drop 实现一点时间完成清理，确保代理设置能被正确恢复
    // 这对于 Ctrl+C 后的清理尤为重要
    std::thread::sleep(std::time::Duration::from_millis(100));

    result
}
