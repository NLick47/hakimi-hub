use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{debug, warn};

const MAX_IPS_PER_DOMAIN: usize = 10;
const PERSIST_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct WorkingIpStore {
    data: Arc<std::sync::RwLock<HashMap<String, Vec<IpAddr>>>>,
    file_path: PathBuf,
    last_persist: Arc<std::sync::RwLock<Instant>>,
}

impl Default for WorkingIpStore {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkingIpStore {
    pub fn new() -> Self {
        let file_path = Self::get_file_path();

        let data = match std::fs::read_to_string(&file_path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
                warn!("解析历史 IP 文件失败: {}", e);
                HashMap::new()
            }),
            Err(_) => HashMap::new(),
        };

        debug!("加载历史成功 IP: {} 个域名", data.len());

        Self {
            data: Arc::new(std::sync::RwLock::new(data)),
            file_path,
            last_persist: Arc::new(std::sync::RwLock::new(Instant::now())),
        }
    }

    fn get_file_path() -> PathBuf {
        if let Some(config_dir) = dirs::config_dir() {
            let dir = config_dir.join("hakimi-hub");
            let _ = std::fs::create_dir_all(&dir);
            dir.join("working_ips.json")
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("working_ips.json")
        }
    }

    pub fn get(&self, domain: &str) -> Vec<IpAddr> {
        self.data
            .read()
            .map(|d| d.get(domain).cloned().unwrap_or_default())
            .unwrap_or_default()
    }

    pub fn record_success(&self, domain: &str, ip: IpAddr) {
        let should_persist = {
            let Ok(mut data) = self.data.write() else {
                warn!("获取写锁失败");
                return;
            };

            let ips = data.entry(domain.to_string()).or_default();
            if let Some(pos) = ips.iter().position(|&x| x == ip) {
                ips.remove(pos);
            }
            ips.insert(0, ip);
            ips.truncate(MAX_IPS_PER_DOMAIN);

            let Ok(last) = self.last_persist.write() else {
                return;
            };
            last.elapsed() >= PERSIST_INTERVAL
        };

        if should_persist {
            self.persist();
        }
    }

    pub fn persist(&self) {
        let data_clone = self
            .data
            .read()
            .map(|d| d.clone())
            .unwrap_or_default();

        if let Ok(mut last) = self.last_persist.write() {
            *last = Instant::now();
        }

        let file_path = self.file_path.clone();
        std::thread::spawn(move || {
            if let Err(e) = std::fs::write(&file_path, serde_json::to_string(&data_clone).unwrap_or_default()) {
                warn!("写入历史 IP 文件失败: {}", e);
            }
        });
    }
}