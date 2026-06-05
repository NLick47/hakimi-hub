use anyhow::Result;
use std::path::PathBuf;
use tracing::{debug, info, warn};

use crate::core::config::AppConfig;

/// Git configuration manager for safe proxy setup
pub struct GitConfig {
    proxy_port: u16,
    ca_cert_path: PathBuf,
    original_proxy: Option<String>,
    original_ssl_ca_info: Option<String>,
}

impl GitConfig {
    pub fn new(config: &AppConfig) -> Self {
        Self {
            proxy_port: config.proxy.port,
            ca_cert_path: crate::utils::paths::ca_cert_path(),
            original_proxy: None,
            original_ssl_ca_info: None,
        }
    }

    /// Safely configure Git to use the proxy
    /// Uses sslCAInfo instead of sslVerify=false for security
    pub fn setup(&mut self) -> Result<()> {
        // Read current values for backup
        self.original_proxy = self.read_git_config("http.https://github.com/.proxy");
        self.original_ssl_ca_info = self.read_git_config("http.https://github.com/.sslCAInfo");

        // Set proxy for GitHub only (not global)
        let proxy_url = format!("http://127.0.0.1:{}", self.proxy_port);
        self.run_git_config("http.https://github.com/.proxy", &proxy_url)?;

        // Set SSL CA info to trust our CA certificate (instead of disabling SSL verify)
        if self.ca_cert_path.exists() {
            self.run_git_config(
                "http.https://github.com/.sslCAInfo",
                &self.ca_cert_path.to_string_lossy(),
            )?;
            info!(
                "Git configured to trust local CA at {}",
                self.ca_cert_path.display()
            );
        } else {
            warn!(
                "CA certificate not found at {}, Git SSL verification may fail",
                self.ca_cert_path.display()
            );
        }

        // Also configure for githubusercontent.com
        self.run_git_config("http.https://raw.githubusercontent.com/.proxy", &proxy_url)?;
        self.run_git_config(
            "http.https://raw.githubusercontent.com/.sslCAInfo",
            &self.ca_cert_path.to_string_lossy(),
        )?;

        info!("Git proxy configured for GitHub");
        Ok(())
    }

    /// Restore original Git configuration
    pub fn teardown(&self) -> Result<()> {
        // Restore or remove proxy setting
        match &self.original_proxy {
            Some(value) => {
                self.run_git_config("http.https://github.com/.proxy", value)?;
                self.run_git_config("http.https://raw.githubusercontent.com/.proxy", value)?;
            }
            None => {
                self.run_git_config_unset("http.https://github.com/.proxy")?;
                self.run_git_config_unset("http.https://raw.githubusercontent.com/.proxy")?;
            }
        }

        // Restore or remove sslCAInfo
        match &self.original_ssl_ca_info {
            Some(value) => {
                self.run_git_config("http.https://github.com/.sslCAInfo", value)?;
                self.run_git_config("http.https://raw.githubusercontent.com/.sslCAInfo", value)?;
            }
            None => {
                self.run_git_config_unset("http.https://github.com/.sslCAInfo")?;
                self.run_git_config_unset("http.https://raw.githubusercontent.com/.sslCAInfo")?;
            }
        }

        info!("Git configuration restored");
        Ok(())
    }

    /// Read a Git config value
    fn read_git_config(&self, key: &str) -> Option<String> {
        let output = std::process::Command::new("git")
            .args(["config", "--global", "--get", key])
            .output()
            .ok()?;

        if output.status.success() {
            String::from_utf8(output.stdout)
                .ok()
                .map(|s| s.trim().to_string())
        } else {
            None
        }
    }

    /// Set a Git config value
    fn run_git_config(&self, key: &str, value: &str) -> Result<()> {
        debug!("git config --global {} {}", key, value);

        let output = std::process::Command::new("git")
            .args(["config", "--global", key, value])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git config failed: {}", stderr);
        }

        Ok(())
    }

    /// Unset a Git config value
    fn run_git_config_unset(&self, key: &str) -> Result<()> {
        debug!("git config --global --unset {}", key);

        let _ = std::process::Command::new("git")
            .args(["config", "--global", "--unset", key])
            .output();

        Ok(())
    }
}

/// Configure Git (standalone function for CLI)
pub fn setup(config: &AppConfig) -> Result<()> {
    let mut git_config = GitConfig::new(config);
    git_config.setup()
}

/// Restore Git configuration (standalone function for CLI)
pub fn teardown() -> Result<()> {
    // Create a minimal config just for teardown
    let git_config = GitConfig {
        proxy_port: 2830,
        ca_cert_path: crate::utils::paths::ca_cert_path(),
        original_proxy: None,
        original_ssl_ca_info: None,
    };
    git_config.teardown()
}
