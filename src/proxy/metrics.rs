use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct Metrics {
    pub active_connections: AtomicU64,
    pub total_connections: AtomicU64,
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,
    // 用于计算速率的即时计数器（每秒重置）
    instant_bytes_sent: AtomicU64,
    instant_bytes_received: AtomicU64,
    last_reset: std::sync::Mutex<Instant>,
    pub mitm_connections: AtomicU64,
    pub tunnel_connections: AtomicU64,
    pub direct_connections: AtomicU64,
    pub doh_connections: AtomicU64,
    pub direct_fallbacks: AtomicU64,
    pub routing_cache_hits: AtomicU64,
    pub routing_cache_misses: AtomicU64,
}

impl Metrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            active_connections: AtomicU64::new(0),
            total_connections: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
            instant_bytes_sent: AtomicU64::new(0),
            instant_bytes_received: AtomicU64::new(0),
            last_reset: std::sync::Mutex::new(Instant::now()),
            mitm_connections: AtomicU64::new(0),
            tunnel_connections: AtomicU64::new(0),
            direct_connections: AtomicU64::new(0),
            doh_connections: AtomicU64::new(0),
            direct_fallbacks: AtomicU64::new(0),
            routing_cache_hits: AtomicU64::new(0),
            routing_cache_misses: AtomicU64::new(0),
        })
    }

    pub fn inc_active(&self) {
        self.active_connections.fetch_add(1, Ordering::Relaxed);
        self.total_connections.fetch_add(1, Ordering::Relaxed);
    }

    pub fn dec_active(&self) {
        self.active_connections.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn add_bytes_sent(&self, n: u64) {
        self.bytes_sent.fetch_add(n, Ordering::Relaxed);
        self.instant_bytes_sent.fetch_add(n, Ordering::Relaxed);
    }

    pub fn add_bytes_received(&self, n: u64) {
        self.bytes_received.fetch_add(n, Ordering::Relaxed);
        self.instant_bytes_received.fetch_add(n, Ordering::Relaxed);
    }

    pub fn inc_mitm(&self) {
        self.mitm_connections.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_tunnel(&self) {
        self.tunnel_connections.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_direct(&self) {
        self.direct_connections.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_doh(&self) {
        self.doh_connections.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_direct_fallback(&self) {
        self.direct_fallbacks.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_routing_cache_hit(&self) {
        self.routing_cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_routing_cache_miss(&self) {
        self.routing_cache_misses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            active_connections: self.active_connections.load(Ordering::Relaxed),
            total_connections: self.total_connections.load(Ordering::Relaxed),
            bytes_sent: self.bytes_sent.load(Ordering::Relaxed),
            bytes_received: self.bytes_received.load(Ordering::Relaxed),
            mitm_connections: self.mitm_connections.load(Ordering::Relaxed),
            tunnel_connections: self.tunnel_connections.load(Ordering::Relaxed),
            direct_connections: self.direct_connections.load(Ordering::Relaxed),
            doh_connections: self.doh_connections.load(Ordering::Relaxed),
            direct_fallbacks: self.direct_fallbacks.load(Ordering::Relaxed),
            routing_cache_hits: self.routing_cache_hits.load(Ordering::Relaxed),
            routing_cache_misses: self.routing_cache_misses.load(Ordering::Relaxed),
        }
    }

    /// 获取实时速率（每秒字节数），并重置即时计数器
    pub fn get_rate_and_reset(&self) -> (u64, u64) {
        // 获取即时字节数
        let sent = self.instant_bytes_sent.swap(0, Ordering::Relaxed);
        let recv = self.instant_bytes_received.swap(0, Ordering::Relaxed);

        // 计算实际经过的时间
        let elapsed = if let Ok(mut last) = self.last_reset.lock() {
            let now = Instant::now();
            let elapsed = now.duration_since(*last);
            *last = now;
            elapsed
        } else {
            Duration::from_secs(1)
        };

        // 计算每秒速率
        let secs = elapsed.as_secs_f64().max(0.001);
        let rate_sent = (sent as f64 / secs) as u64;
        let rate_recv = (recv as f64 / secs) as u64;

        (rate_sent, rate_recv)
    }
}

#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub active_connections: u64,
    pub total_connections: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub mitm_connections: u64,
    pub tunnel_connections: u64,
    pub direct_connections: u64,
    pub doh_connections: u64,
    pub direct_fallbacks: u64,
    pub routing_cache_hits: u64,
    pub routing_cache_misses: u64,
}
