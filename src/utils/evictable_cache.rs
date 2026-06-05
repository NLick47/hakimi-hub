//! 可淘汰缓存封装
//!
//! 基于 DashMap 的惰性淘汰缓存，采用惰性淘汰策略：
//! - TTL 过期：在 `get` 时检查单个条目，过期则淘汰
//! - LRU 容量淘汰：在 `insert` 时仅当超过容量才淘汰
//!
//! 性能优化：使用 LRU 索引实现 O(1) 淘汰，避免全量遍历排序。
//! - 数据存储：DashMap（并发安全）
//! - LRU 顺序：RwLock<LruCache>（O(1) 淘汰）

use std::borrow::Borrow;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use lru::LruCache;
use tracing::debug;

/// 带访问元数据的缓存条目包装
#[derive(Debug, Clone)]
pub struct Entry<V> {
    /// 实际数据
    pub value: V,
    /// 最后访问时间
    pub last_accessed: Instant,
    /// 创建时间（用于 TTL）
    pub created_at: Instant,
}

/// 基于 DashMap 的可淘汰并发缓存（惰性淘汰 + O(1) LRU）
///
/// - TTL 过期：`get` 时惰性检查单个条目
/// - LRU O(1) 容量淘汰：`insert` 时使用 LRU 索引快速找到淘汰候选
/// - `get()` 自动更新 last_accessed 和 LRU 顺序
pub struct EvictableCache<K, V> {
    /// 数据存储（并发安全）
    inner: DashMap<K, Entry<V>>,
    /// LRU 索引：维护访问顺序，用于 O(1) 淘汰
    /// 注意：这里只存储 key，实际数据在 inner 中
    pub(crate) lru_index: RwLock<LruCache<K, ()>>,
    max_entries: usize,
    name: &'static str,
    /// TTL 持续时间（可选）
    ttl: Option<Duration>,
    /// 上次淘汰时间戳（纳秒），用于限流
    last_evict_ns: AtomicU64,
}

impl<K, V> EvictableCache<K, V>
where
    K: Hash + Eq + Clone + Debug + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    /// 创建新的惰性淘汰缓存
    pub fn new(max_entries: usize, name: &'static str) -> Self {
        Self {
            inner: DashMap::new(),
            lru_index: RwLock::new(LruCache::unbounded()),
            max_entries,
            name,
            ttl: None,
            last_evict_ns: AtomicU64::new(0),
        }
    }

    /// 创建带 TTL 的缓存
    pub fn with_ttl(max_entries: usize, name: &'static str, ttl: Duration) -> Self {
        Self {
            inner: DashMap::new(),
            lru_index: RwLock::new(LruCache::unbounded()),
            max_entries,
            name,
            ttl: Some(ttl),
            last_evict_ns: AtomicU64::new(0),
        }
    }

    /// 获取条目并更新 last_accessed（惰性 TTL 检查）
    ///
    /// 使用 DashMap 的 entry API 或 update 方法来原子地更新 last_accessed，
    /// 消除原来 get() 和 get_mut() 之间的竞态窗口。
    pub fn get<Q>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let now = Instant::now();
        let mut result: Option<V> = None;
        let mut expired = false;
        let mut key_clone: Option<K> = None;

        // 使用 entry API 进行原子操作
        if let Some(mut entry) = self.inner.get_mut(key) {
            // 惰性 TTL 检查
            if let Some(ttl) = self.ttl {
                if entry.created_at.elapsed() > ttl {
                    // 标记为过期，需要在释放引用后删除
                    expired = true;
                }
            }

            if !expired {
                // 更新访问时间并克隆值
                entry.last_accessed = now;
                result = Some(entry.value.clone());
                key_clone = Some(entry.key().clone());
            }
        }

        // 如果过期，在释放引用后删除
        if expired {
            self.inner.remove(key);
            // 从 LRU 索引中移除
            if let Ok(mut lru) = self.lru_index.write() {
                lru.pop(key);
            }
            debug!("{} 缓存惰性过期淘汰", self.name);
        } else if let Some(k) = key_clone {
            // 更新 LRU 顺序（将 key 移到最前面）
            if let Ok(mut lru) = self.lru_index.write() {
                lru.put(k, ());
            }
        }

        result
    }

    /// 获取 Entry（含元数据），更新 last_accessed（惰性 TTL 检查）
    ///
    /// 使用 get_mut 进行原子操作，消除竞态窗口。
    pub fn get_entry<Q>(&self, key: &Q) -> Option<Entry<V>>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let now = Instant::now();
        let mut expired = false;
        let mut result: Option<Entry<V>> = None;
        let mut key_clone: Option<K> = None;

        // 使用 get_mut 进行原子操作
        if let Some(mut entry) = self.inner.get_mut(key) {
            // 惰性 TTL 检查
            if let Some(ttl) = self.ttl {
                if entry.created_at.elapsed() > ttl {
                    expired = true;
                }
            }

            if !expired {
                key_clone = Some(entry.key().clone());
                result = Some(Entry {
                    value: entry.value.clone(),
                    last_accessed: now,
                    created_at: entry.created_at,
                });
                entry.last_accessed = now;
            }
        }

        if expired {
            self.inner.remove(key);
            if let Ok(mut lru) = self.lru_index.write() {
                lru.pop(key);
            }
            debug!("{} 缓存惰性过期淘汰", self.name);
        } else if let Some(k) = key_clone {
            // 更新 LRU 顺序
            if let Ok(mut lru) = self.lru_index.write() {
                lru.put(k, ());
            }
        }

        result
    }

    /// 获取值的引用（不更新 last_accessed，不做 TTL 检查）
    pub fn peek<Q>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.inner.get(key).map(|e| e.value.clone())
    }

    /// 插入条目（O(1) 惰性容量淘汰）
    pub fn insert(&self, key: K, value: V) {
        let now = Instant::now();
        self.inner.insert(
            key.clone(),
            Entry {
                value,
                last_accessed: now,
                created_at: now,
            },
        );

        // 更新 LRU 索引
        if let Ok(mut lru) = self.lru_index.write() {
            lru.put(key, ());
        }

        // 仅当超过容量时才执行 LRU 淘汰
        self.evict_lru_if_needed();
    }

    /// 移除条目
    pub fn remove<Q>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        // 从 LRU 索引中移除
        if let Ok(mut lru) = self.lru_index.write() {
            lru.pop(key);
        }
        self.inner.remove(key).map(|(_, e)| e.value)
    }

    /// 是否包含（不做 TTL 检查）
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.inner.contains_key(key)
    }

    /// 条目数
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// 清空
    pub fn clear(&self) {
        self.inner.clear();
        if let Ok(mut lru) = self.lru_index.write() {
            lru.clear();
        }
    }

    /// O(1) LRU 淘汰：仅当超过容量时执行
    fn evict_lru_if_needed(&self) {
        let excess = self.inner.len().saturating_sub(self.max_entries);
        if excess == 0 {
            return;
        }

        // 简单限流：避免高并发时重复淘汰
        let now_ns = instant_to_nanos(Instant::now());
        let last = self.last_evict_ns.load(Ordering::Relaxed);
        // 最小间隔 1ms
        if now_ns.saturating_sub(last) < 1_000_000 {
            return;
        }
        self.last_evict_ns.store(now_ns, Ordering::Relaxed);

        // O(1) LRU 淘汰：从 LRU 索引尾部弹出最久未使用的 key
        if let Ok(mut lru) = self.lru_index.write() {
            for _ in 0..excess {
                // LruCache 的 pop_lru 返回最久未使用的条目
                if let Some((old_key, _)) = lru.pop_lru() {
                    self.inner.remove(&old_key);
                    debug!("{} 缓存 O(1) LRU 淘汰: {:?}", self.name, old_key);
                } else {
                    break; // LRU 索引已空
                }
            }
        }
    }

    /// 手动执行淘汰（保留 API 兼容性，但改为惰性风格）
    ///
    /// 注意：此方法现在主要用于主动清理场景，
    /// 正常情况下 get/insert 会自动处理淘汰。
    pub fn evict(&self, ttl: Option<Duration>) {
        let now = Instant::now();

        // TTL 过期清理（需要显式调用时才全量扫描）
        if let Some(ttl) = ttl {
            let expired: Vec<K> = self
                .inner
                .iter()
                .filter(|e| now.duration_since(e.created_at) > ttl)
                .map(|e| e.key().clone())
                .collect();
            for key in expired {
                self.inner.remove(&key);
                if let Ok(mut lru) = self.lru_index.write() {
                    lru.pop(&key);
                }
                debug!("{} 缓存过期淘汰: {:?}", self.name, key);
            }
        }

        // LRU 容量淘汰
        self.evict_lru_if_needed();
    }

    /// 底层 DashMap 引用（用于需要直接操作的场景）
    pub fn dashmap(&self) -> &DashMap<K, Entry<V>> {
        &self.inner
    }
}

/// 将 Instant 转换为纳秒（相对值）
///
/// 使用 Instant 的相对时间，避免 SystemTime 受系统时钟调整影响。
/// 通过记录一个基准时间点，计算 Instant 相对于该基准的纳秒偏移。
fn instant_to_nanos(t: Instant) -> u64 {
    // 使用一个静态基准时间，所有 Instant 都相对于这个基准计算
    // 这避免了 SystemTime 受系统时间调整影响的问题
    static BASE: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let base = BASE.get_or_init(Instant::now);

    // 计算相对于基准的纳秒偏移
    // 注意：Instant::duration_since 可能因时钟调整返回负值，
    // 但在我们的使用场景（限流）中，这足够稳定
    t.duration_since(*base).as_nanos() as u64
}

/// 简化版：仅 LRU 容量淘汰（无 TTL）
///
/// 通过组合 EvictableCache 实现，insert 时自动惰性淘汰。
/// 适用于证书缓存等不需要过期的场景。
pub struct SimpleEvictableCache<K, V> {
    inner: EvictableCache<K, V>,
}

impl<K, V> SimpleEvictableCache<K, V>
where
    K: Hash + Eq + Clone + Debug + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    pub fn new(max_entries: usize, name: &'static str) -> Self {
        Self {
            inner: EvictableCache::new(max_entries, name),
        }
    }

    pub fn get<Q>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.inner.get(key)
    }

    pub fn insert(&self, key: K, value: V) {
        // insert 内部已包含 O(1) LRU 淘汰
        self.inner.insert(key, value);
    }

    pub fn remove<Q>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.inner.remove(key)
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn clear(&self) {
        self.inner.clear();
    }

    pub fn dashmap(&self) -> &DashMap<K, Entry<V>> {
        self.inner.dashmap()
    }
}

/// 带 TTL 的惰性淘汰缓存（简化 API）
pub struct TtlEvictableCache<K, V> {
    inner: EvictableCache<K, V>,
}

impl<K, V> TtlEvictableCache<K, V>
where
    K: Hash + Eq + Clone + Debug + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    pub fn new(max_entries: usize, name: &'static str, ttl: Duration) -> Self {
        Self {
            inner: EvictableCache::with_ttl(max_entries, name, ttl),
        }
    }

    /// 获取条目（惰性 TTL 检查）
    pub fn get<Q>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.inner.get(key)
    }

    pub fn insert(&self, key: K, value: V) {
        self.inner.insert(key, value);
    }

    pub fn remove<Q>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.inner.remove(key)
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn clear(&self) {
        self.inner.clear();
    }

    pub fn dashmap(&self) -> &DashMap<K, Entry<V>> {
        self.inner.dashmap()
    }
}
