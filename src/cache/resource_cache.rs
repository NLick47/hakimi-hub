use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;
use tracing::{debug, info, warn};

use crate::core::config::CacheConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheMetadata {
    /// 原始 URL
    pub url: String,
    /// Content-Type
    pub content_type: String,
    /// ETag
    pub etag: Option<String>,
    /// Last-Modified
    pub last_modified: Option<String>,
    /// 创建时间
    pub created_at: u64,
    /// 最后访问时间
    pub last_accessed: u64,
    /// 文件大小
    pub file_size: u64,
    /// 相对于 cache 目录的文件路径
    pub file_path: String,
}

#[derive(Debug, Clone)]
pub struct CachedResource {
    /// Content-Type
    pub content_type: String,
    /// 资源数据
    pub data: Vec<u8>,
    /// ETag
    pub etag: Option<String>,
    /// Last-Modified
    pub last_modified: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CacheStats {
    /// 缓存目录路径
    pub cache_dir: PathBuf,
    /// 总缓存大小（字节）
    pub total_size: u64,
    /// 文件数量
    pub file_count: usize,
    /// 最早缓存时间
    pub oldest: Option<u64>,
    /// 最近缓存时间
    pub newest: Option<u64>,
}

fn is_github_domain(url: &str) -> bool {
    let url_lower = url.to_lowercase();

    let github_domains = [
        "githubusercontent.com",
        "githubassets.com",
        "avatars.githubusercontent.com",
        "objects.githubusercontent.com",
        "release-assets.githubusercontent.com",
        "github.com",
    ];

    for domain in github_domains {
        if url_lower.contains(domain) {
            return true;
        }
    }

    false
}

fn is_image_content_type(content_type: &str) -> bool {
    let ct_lower = content_type.to_lowercase();
    ct_lower.starts_with("image/")
}

fn has_image_extension(url: &str) -> bool {
    let url_lower = url.to_lowercase();
    let path = if let Some(pos) = url_lower.find('?') {
        &url_lower[..pos]
    } else if let Some(pos) = url_lower.find('#') {
        &url_lower[..pos]
    } else {
        &url_lower
    };

    let image_extensions = [
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".ico", ".bmp", ".avif",
    ];
    image_extensions.iter().any(|ext| path.ends_with(ext))
}

pub struct ResourceCache {
    /// 缓存目录
    cache_dir: PathBuf,
    /// 元数据文件路径
    metadata_path: PathBuf,
    /// 缓存元数据
    metadata: Arc<DashMap<String, CacheMetadata>>,
    /// 配置
    config: CacheConfig,
    /// 元数据是否需要保存
    metadata_dirty: std::sync::atomic::AtomicBool,
}

impl ResourceCache {
    pub fn new(cache_dir: PathBuf, config: CacheConfig) -> Self {
        let metadata_path = cache_dir.join("metadata.json");
        let metadata = Arc::new(Self::load_metadata(&metadata_path).unwrap_or_default());

        Self {
            cache_dir,
            metadata_path,
            metadata,
            config,
            metadata_dirty: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn load_metadata(path: &Path) -> anyhow::Result<DashMap<String, CacheMetadata>> {
        if !path.exists() {
            return Ok(DashMap::new());
        }

        let content = std::fs::read_to_string(path)?;
        let map: std::collections::HashMap<String, CacheMetadata> = serde_json::from_str(&content)?;
        let dashmap = DashMap::with_capacity(map.len());
        for (k, v) in map {
            dashmap.insert(k, v);
        }
        debug!("已加载 {} 条缓存元数据", dashmap.len());
        Ok(dashmap)
    }

    async fn save_metadata_async(&self) -> anyhow::Result<()> {
        let metadata: std::collections::HashMap<String, CacheMetadata> = self
            .metadata
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();

        let content = serde_json::to_string_pretty(&metadata)?;
        fs::write(&self.metadata_path, content).await?;
        debug!("已保存 {} 条缓存元数据", metadata.len());
        Ok(())
    }

    fn url_to_key(url: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(url.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    fn get_file_path(&self, key: &str) -> PathBuf {
        if key.len() >= 2 {
            let subdir = &key[..2];
            self.cache_dir.join(subdir).join(&key[2..])
        } else {
            self.cache_dir.join(key)
        }
    }

    pub fn is_cacheable(url: &str, content_type: &str, _content_length: Option<u64>) -> bool {
        if !is_github_domain(url) {
            return false;
        }

        if is_image_content_type(content_type) {
            return true;
        }

        has_image_extension(url)
    }

    pub fn is_cacheable_url(url: &str) -> bool {
        is_github_domain(url) && has_image_extension(url)
    }

    pub async fn get(&self, url: &str) -> Option<CachedResource> {
        if !self.config.enabled {
            return None;
        }

        let key = Self::url_to_key(url);

        let (metadata_clone, created_at) = {
            let metadata = self.metadata.get(&key)?;
            (metadata.value().clone(), metadata.created_at)
        };

        let now = current_timestamp();
        let ttl_seconds = self.config.ttl_days * 24 * 3600;
        if now - created_at > ttl_seconds {
            debug!("缓存已过期: {}", url);
            let metadata_clone = Arc::clone(&self.metadata);
            let cache_dir = self.cache_dir.clone();
            let key_clone = key.clone();
            tokio::spawn(async move {
                Self::remove_async_static(metadata_clone, cache_dir, key_clone).await;
            });
            return None;
        }

        let file_path = self.get_file_path(&key);
        let data = match fs::read(&file_path).await {
            Ok(d) => d,
            Err(e) => {
                warn!("读取缓存文件失败: {}", e);
                // 异步删除失效条目
                let metadata = Arc::clone(&self.metadata);
                let cache_dir = self.cache_dir.clone();
                let key_clone = key.clone();
                tokio::spawn(async move {
                    Self::remove_async_static(metadata, cache_dir, key_clone).await;
                });
                return None;
            }
        };

        debug!("缓存命中: {}", url);

        Some(CachedResource {
            content_type: metadata_clone.content_type,
            data,
            etag: metadata_clone.etag,
            last_modified: metadata_clone.last_modified,
        })
    }

    pub async fn put(&self, url: &str, resource: &CachedResource) -> anyhow::Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        if !Self::is_cacheable(url, &resource.content_type, None) {
            return Ok(());
        }

        let max_bytes = (self.config.max_file_size_mb as u64) * 1024 * 1024;
        if resource.data.len() as u64 > max_bytes {
            debug!("资源过大，不缓存: {} bytes", resource.data.len());
            return Ok(());
        }

        let key = Self::url_to_key(url);
        let file_path = self.get_file_path(&key);

        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        fs::write(&file_path, &resource.data).await?;

        let now = current_timestamp();
        let rel_path = file_path.strip_prefix(&self.cache_dir)?;
        let metadata = CacheMetadata {
            url: url.to_string(),
            content_type: resource.content_type.clone(),
            etag: resource.etag.clone(),
            last_modified: resource.last_modified.clone(),
            created_at: now,
            last_accessed: now,
            file_size: resource.data.len() as u64,
            file_path: rel_path.to_string_lossy().to_string(),
        };

        self.metadata.insert(key.clone(), metadata);

        self.metadata_dirty
            .store(true, std::sync::atomic::Ordering::Relaxed);

        let metadata = Arc::clone(&self.metadata);
        let cache_dir = self.cache_dir.clone();
        let config = self.config.clone();
        let metadata_path = self.metadata_path.clone();
        tokio::spawn(async move {
            if let Err(e) =
                Self::evict_if_needed_static(metadata, cache_dir, config, metadata_path).await
            {
                warn!("缓存淘汰失败: {}", e);
            }
        });

        debug!("已缓存: {} ({} bytes)", url, resource.data.len());
        Ok(())
    }

    pub async fn remove(&self, url: &str) {
        let key = Self::url_to_key(url);
        Self::remove_async_static(Arc::clone(&self.metadata), self.cache_dir.clone(), key).await;
    }

    async fn remove_async_static(
        metadata: Arc<DashMap<String, CacheMetadata>>,
        cache_dir: PathBuf,
        key: String,
    ) {
        let file_path = if key.len() >= 2 {
            cache_dir.join(&key[..2]).join(&key[2..])
        } else {
            cache_dir.join(&key)
        };

        let _ = fs::remove_file(&file_path).await;

        metadata.remove(&key);
    }

    pub async fn clear(&self) -> anyhow::Result<usize> {
        let mut count = 0;
        let mut total_size = 0u64;

        let entries: Vec<(String, PathBuf, u64)> = self
            .metadata
            .iter()
            .map(|entry| {
                (
                    entry.key().clone(),
                    self.cache_dir.join(&entry.file_path),
                    entry.file_size,
                )
            })
            .collect();

        for (key, file_path, size) in entries {
            if fs::remove_file(&file_path).await.is_ok() {
                count += 1;
                total_size += size;
            }
            self.metadata.remove(&key);
        }

        self.save_metadata_async().await?;

        info!("已清理 {} 个缓存文件，释放 {} 字节", count, total_size);
        Ok(count)
    }

    pub async fn clear_expired(&self) -> anyhow::Result<usize> {
        let now = current_timestamp();
        let ttl_seconds = self.config.ttl_days * 24 * 3600;

        let expired_entries: Vec<(String, PathBuf)> = self
            .metadata
            .iter()
            .filter(|entry| now - entry.created_at > ttl_seconds)
            .map(|entry| (entry.key().clone(), self.get_file_path(entry.key())))
            .collect();

        let mut count = 0;
        for (key, file_path) in expired_entries {
            if fs::remove_file(&file_path).await.is_ok() {
                count += 1;
            }
            self.metadata.remove(&key);
        }

        if count > 0 {
            self.save_metadata_async().await?;
            info!("已清理 {} 个过期缓存", count);
        }

        Ok(count)
    }

    pub fn stats(&self) -> CacheStats {
        let total_size: u64 = self.metadata.iter().map(|entry| entry.file_size).sum();
        let file_count = self.metadata.len();

        let oldest = self.metadata.iter().map(|entry| entry.created_at).min();
        let newest = self.metadata.iter().map(|entry| entry.created_at).max();

        CacheStats {
            cache_dir: self.cache_dir.clone(),
            total_size,
            file_count,
            oldest,
            newest,
        }
    }

    pub async fn flush(&self) -> anyhow::Result<()> {
        if self
            .metadata_dirty
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            self.save_metadata_async().await?;
            self.metadata_dirty
                .store(false, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(())
    }

    async fn evict_if_needed_static(
        metadata: Arc<DashMap<String, CacheMetadata>>,
        cache_dir: PathBuf,
        config: CacheConfig,
        metadata_path: PathBuf,
    ) -> anyhow::Result<()> {
        let max_bytes = (config.max_size_mb as u64) * 1024 * 1024;

        let current_size: u64 = metadata.iter().map(|entry| entry.file_size).sum();

        if current_size <= max_bytes {
            return Ok(());
        }

        let mut entries: Vec<_> = metadata
            .iter()
            .map(|entry| (entry.key().clone(), entry.last_accessed, entry.file_size))
            .collect();

        entries.sort_by_key(|(_, accessed, _)| *accessed);

        let mut freed = 0u64;
        let mut count = 0;
        let target = current_size - max_bytes;

        for (key, _, size) in entries {
            if freed >= target {
                break;
            }

            let file_path = if key.len() >= 2 {
                cache_dir.join(&key[..2]).join(&key[2..])
            } else {
                cache_dir.join(&key)
            };

            // 直接尝试删除，忽略不存在错误
            if fs::remove_file(&file_path).await.is_ok() {
                freed += size;
                count += 1;
            }

            metadata.remove(&key);
        }

        if count > 0 {
            let meta_map: std::collections::HashMap<String, CacheMetadata> = metadata
                .iter()
                .map(|entry| (entry.key().clone(), entry.value().clone()))
                .collect();
            let content = serde_json::to_string_pretty(&meta_map)?;
            fs::write(&metadata_path, content).await?;
            info!("LRU 淘汰: 已删除 {} 个缓存，释放 {} 字节", count, freed);
        }

        Ok(())
    }
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
