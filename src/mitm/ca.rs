use std::path::Path;
use std::sync::Arc;

use chrono::Datelike;
use once_cell::sync::OnceCell;
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tracing::{debug, info, warn};

// MITM 根 CA，负责签发动态生成的域名证书
pub struct CertificateAuthority {
    key_pair: KeyPair,
    cert: rcgen::Certificate,
    #[allow(dead_code)]
    key_size: usize,
}

impl CertificateAuthority {
    // 有就用旧的，没有就生成新的
    pub async fn load_or_generate(data_dir: &Path, key_size: usize) -> anyhow::Result<Self> {
        let ca_cert_path = data_dir.join("ca.crt.pem");
        let ca_key_path = data_dir.join("ca.key.pem");

        if ca_cert_path.exists() && ca_key_path.exists() {
            info!("正在加载现有 CA 证书");
            return Self::load_from_files(&ca_cert_path, &ca_key_path, key_size);
        }

        info!("正在生成新 CA 证书（{} 位 RSA）", key_size);
        let ca = Self::generate(key_size)?;

        if let Some(parent) = ca_cert_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let cert_pem = ca.cert.pem();
        let key_pem = ca.key_pair.serialize_pem();

        std::fs::write(&ca_cert_path, &cert_pem)?;
        std::fs::write(&ca_key_path, &key_pem)?;

        info!("CA 证书已保存至 {}", ca_cert_path.display());
        info!("CA 私钥已保存至 {}", ca_key_path.display());

        Ok(ca)
    }

    // 从 PEM 文件加载，失败就重新生成
    fn load_from_files(cert_path: &Path, key_path: &Path, key_size: usize) -> anyhow::Result<Self> {
        match Self::load_existing_ca(cert_path, key_path, key_size) {
            Ok(ca) => {
                info!("成功加载已有的 CA 证书");
                Ok(ca)
            }
            Err(e) => {
                warn!("加载已有 CA 证书失败: {}，将重新生成", e);
                Self::generate(key_size)
            }
        }
    }

    // 解析 PEM，重建证书对象
    // 注意：key_size 用传入参数，别硬编码
    fn load_existing_ca(
        _cert_path: &Path,
        key_path: &Path,
        key_size: usize,
    ) -> anyhow::Result<Self> {
        let key_pem = std::fs::read_to_string(key_path)
            .map_err(|e| anyhow::anyhow!("读取 CA 私钥文件失败: {}", e))?;

        let key_pair = KeyPair::from_pem(&key_pem)
            .map_err(|e| anyhow::anyhow!("解析 CA 私钥 PEM 失败: {}", e))?;

        let mut params = CertificateParams::new(Vec::new())
            .map_err(|e| anyhow::anyhow!("创建证书参数失败: {}", e))?;
        params
            .distinguished_name
            .push(DnType::CommonName, "Hakimi Hub CA");
        params
            .distinguished_name
            .push(DnType::OrganizationName, "Hakimi Hub");
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        params.not_after = rcgen::date_time_ymd(2034, 1, 1);

        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| anyhow::anyhow!("签名 CA 证书失败: {}", e))?;

        Ok(Self {
            key_pair,
            cert,
            key_size, // 用参数，别硬编码 4096
        })
    }

    // 生成新的根 CA
    fn generate(key_size: usize) -> anyhow::Result<Self> {
        let mut params = CertificateParams::new(Vec::new())?;
        params
            .distinguished_name
            .push(DnType::CommonName, "Hakimi Hub CA");
        params
            .distinguished_name
            .push(DnType::OrganizationName, "Hakimi Hub");
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        params.not_after = rcgen::date_time_ymd(2034, 1, 1);

        let key_pair = KeyPair::generate()?;
        let cert = params.self_signed(&key_pair)?;

        Ok(Self {
            key_pair,
            cert,
            key_size,
        })
    }

    // 给域名签发证书
    pub fn sign_domain_cert(
        &self,
        domain: &str,
        validity_days: u32,
    ) -> anyhow::Result<CertKeyPair> {
        debug!("正在为 {} 签发证书", domain);

        let mut params = CertificateParams::new(vec![domain.to_string()])?;
        params.distinguished_name.push(DnType::CommonName, domain);

        params
            .subject_alt_names
            .push(rcgen::SanType::DnsName(rcgen::Ia5String::try_from(
                domain.to_string(),
            )?));

        let now = chrono::Utc::now();
        params.not_before = rcgen::date_time_ymd(now.year(), now.month() as u8, now.day() as u8);
        let expires = now + chrono::Duration::days(validity_days as i64);
        params.not_after =
            rcgen::date_time_ymd(expires.year(), expires.month() as u8, expires.day() as u8);

        let domain_key_pair = KeyPair::generate()?;
        let domain_cert = params.signed_by(&domain_key_pair, &self.cert, &self.key_pair)?;

        Ok(CertKeyPair::new(
            domain_cert.pem(),
            domain_key_pair.serialize_pem(),
            domain.to_string(),
        ))
    }

    // 导出 PEM 格式
    pub fn export_ca_cert_pem(&self) -> String {
        self.cert.pem()
    }

    // DER 格式
    pub fn ca_cert_der(&self) -> Vec<u8> {
        self.cert.der().to_vec()
    }

    // 测试用
    #[cfg(test)]
    pub fn key_size(&self) -> usize {
        self.key_size
    }
}

// 签发的域名证书 + 私钥，带缓存
pub struct CertKeyPair {
    pub cert_pem: String,
    pub key_pem: String,
    pub domain: String,
    // 缓存 rustls 的 CertifiedKey，避免每次握手都解析 PEM
    cached_certified_key: OnceCell<Arc<rustls::sign::CertifiedKey>>,
}

impl CertKeyPair {
    pub fn new(cert_pem: String, key_pem: String, domain: String) -> Self {
        Self {
            cert_pem,
            key_pem,
            domain,
            cached_certified_key: OnceCell::new(),
        }
    }

    // 获取 rustls 的 CertifiedKey（带缓存）
    // 直接给 ServerConfig 用
    pub fn certified_key(&self) -> anyhow::Result<Arc<rustls::sign::CertifiedKey>> {
        self.cached_certified_key
            .get_or_try_init(|| {
                let mut certs = Vec::new();
                for cert in rustls_pemfile::certs(&mut self.cert_pem.as_bytes()) {
                    certs.push(cert?);
                }

                let key = rustls_pemfile::private_key(&mut self.key_pem.as_bytes())
                    .map_err(|e| anyhow::anyhow!("解析私钥失败: {}", e))?
                    .ok_or_else(|| anyhow::anyhow!("PEM 中未找到私钥"))?;

                let signing_key = rustls::crypto::ring::sign::any_supported_type(&key)?;
                let certified_key = rustls::sign::CertifiedKey::new(certs, signing_key);

                Ok(Arc::new(certified_key))
            })
            .cloned()
    }

    // 从缓存提取 DER 证书
    pub fn cert_der(&self) -> anyhow::Result<Vec<CertificateDer<'static>>> {
        Ok(self.certified_key()?.cert.clone())
    }

    // 解析私钥
    pub fn key_der(&self) -> anyhow::Result<PrivateKeyDer<'static>> {
        rustls_pemfile::private_key(&mut self.key_pem.as_bytes())
            .map_err(|e| anyhow::anyhow!("解析私钥失败: {}", e))?
            .ok_or_else(|| anyhow::anyhow!("PEM 中未找到私钥"))
    }
}
