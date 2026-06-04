use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing::debug;

use crate::mitm::ca::{CertificateAuthority, CertKeyPair};
use crate::utils::evictable_cache::SimpleEvictableCache;

// 域名证书缓存，内存 + 磁盘两级
pub struct CertCache {
    ca: Arc<CertificateAuthority>,
    memory: SimpleEvictableCache<String, Arc<CertKeyPair>>,
    disk_dir: PathBuf,
    validity_days: u32,
}

impl CertCache {
    pub fn new(
        ca: Arc<CertificateAuthority>,
        disk_dir: PathBuf,
        max_entries: usize,
        validity_days: u32,
    ) -> Self {
        // 确保磁盘缓存目录存在
        if let Err(e) = std::fs::create_dir_all(&disk_dir) {
            debug!("无法创建证书缓存目录: {}", e);
        }

        Self {
            ca,
            memory: SimpleEvictableCache::new(max_entries, "cert_cache"),
            disk_dir,
            validity_days,
        }
    }

    // 获取证书，没有就生成
    pub async fn get_or_generate(&self, domain: &str) -> anyhow::Result<Arc<CertKeyPair>> {
        // 先查内存
        if let Some(cert) = self.memory.get(domain) {
            debug!("证书内存缓存命中: {}", domain);
            return Ok(cert);
        }

        // 再查磁盘
        let disk_path = self.disk_path(domain);
        if disk_path.exists() {
            if let Ok(cert) = self.load_from_disk(&disk_path, domain) {
                self.memory.insert(domain.to_string(), cert.clone());
                debug!("证书磁盘缓存命中: {}", domain);
                return Ok(cert);
            }
        }

        // 都没有，生成新的
        debug!("正在为 {} 生成证书", domain);
        let cert = self.ca.sign_domain_cert(domain, self.validity_days)?;
        let cert = Arc::new(cert);

        // 存磁盘
        if let Err(e) = self.save_to_disk(&disk_path, &cert) {
            debug!("保存证书到磁盘失败: {}", e);
        }

        // 存内存
        self.memory.insert(domain.to_string(), cert.clone());

        Ok(cert)
    }

    // 域名转文件名，非法字符换成下划线
    fn disk_path(&self, domain: &str) -> PathBuf {
        let safe_name: String = domain
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' })
            .collect();
        self.disk_dir.join(format!("{}.pem", safe_name))
    }

    // 证书 + 私钥写到一个文件里
    fn save_to_disk(&self, path: &Path, cert: &CertKeyPair) -> std::io::Result<()> {
        let content = format!("{}\n{}", cert.cert_pem, cert.key_pem);
        std::fs::write(path, content)
    }

    // 从磁盘加载
    fn load_from_disk(&self, path: &Path, domain: &str) -> std::io::Result<Arc<CertKeyPair>> {
        let content = std::fs::read_to_string(path)?;

        // 找证书部分
        let cert_start = content
            .find("-----BEGIN CERTIFICATE-----")
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "No certificate found"))?;
        let cert_end = content
            .find("-----END CERTIFICATE-----")
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "No certificate end found"))?
            + "-----END CERTIFICATE-----".len();

        // 找私钥部分（PRIVATE KEY 和 RSA PRIVATE KEY 都支持）
        let key_start = content
            .find("-----BEGIN PRIVATE KEY-----")
            .or_else(|| content.find("-----BEGIN RSA PRIVATE KEY-----"))
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "No private key found"))?;
        let key_end = content[key_start..]
            .find("-----END PRIVATE KEY-----")
            .map(|e| key_start + e + "-----END PRIVATE KEY-----".len())
            .or_else(|| {
                content[key_start..]
                    .find("-----END RSA PRIVATE KEY-----")
                    .map(|e| key_start + e + "-----END RSA PRIVATE KEY-----".len())
            })
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "No private key end found"))?;

        Ok(Arc::new(CertKeyPair::new(
            content[cert_start..cert_end].to_string(),
            content[key_start..key_end].to_string(),
            domain.to_string(),
        )))
    }
}

