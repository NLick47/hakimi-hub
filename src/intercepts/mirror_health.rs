//! 镜像健康追踪器
//!
//! 跟踪镜像站的成功失败情况，失败次数过多时自动禁用，超时后恢复。

use std::time::{Duration, Instant};

use dashmap::DashMap;
use tracing::{debug, warn};

/// 单个镜像的健康状态
#[derive(Debug, Clone)]
struct MirrorHealth {
    consecutive_failures: u32,
    total_successes: u64,
    total_failures: u64,
    last_response_time_ms: Option<f64>,
    avg_response_time_ms: f64,
    response_time_count: u64,
    disabled_at: Option<Instant>,
}

impl Default for MirrorHealth {
    fn default() -> Self {
        Self {
            consecutive_failures: 0,
            total_successes: 0,
            total_failures: 0,
            last_response_time_ms: None,
            avg_response_time_ms: 0.0,
            response_time_count: 0,
            disabled_at: None,
        }
    }
}

/// 镜像健康追踪器
pub struct MirrorHealthTracker {
    health: DashMap<String, MirrorHealth>,
    failure_threshold: u32,
    recovery_timeout: Duration,
}

impl MirrorHealthTracker {
    /// 创建追踪器，可配置失败阈值和恢复时间
    pub fn new(failure_threshold: u32, recovery_timeout_secs: u64) -> Self {
        Self {
            health: DashMap::new(),
            failure_threshold: failure_threshold.max(1),
            recovery_timeout: Duration::from_secs(recovery_timeout_secs),
        }
    }

    /// 使用默认配置：连续失败3次禁用，5分钟后恢复
    pub fn default_tracker() -> Self {
        Self::new(3, 300)
    }

    /// 记录请求成功
    pub fn record_success(&self, mirror: &str, response_time_ms: f64) {
        let mut health = self.health.entry(mirror.to_string()).or_default();

        health.consecutive_failures = 0;
        health.total_successes += 1;
        health.last_response_time_ms = Some(response_time_ms);

        // 指数移动平均，平滑因子 0.3
        if health.response_time_count == 0 {
            health.avg_response_time_ms = response_time_ms;
        } else {
            health.avg_response_time_ms = 0.3 * response_time_ms + 0.7 * health.avg_response_time_ms;
        }
        health.response_time_count += 1;

        if health.disabled_at.is_some() {
            debug!("镜像 {} 恢复可用", mirror);
            health.disabled_at = None;
        }

        debug!(
            "镜像 {} 成功 (耗时: {:.0}ms, 平均: {:.0}ms)",
            mirror, response_time_ms, health.avg_response_time_ms
        );
    }

    /// 记录请求失败
    pub fn record_failure(&self, mirror: &str) {
        let mut health = self.health.entry(mirror.to_string()).or_default();

        health.consecutive_failures += 1;
        health.total_failures += 1;

        if health.consecutive_failures >= self.failure_threshold && health.disabled_at.is_none() {
            health.disabled_at = Some(Instant::now());
            warn!(
                "镜像 {} 连续失败 {} 次，已禁用（{}秒后重试）",
                mirror,
                health.consecutive_failures,
                self.recovery_timeout.as_secs()
            );
        } else {
            debug!(
                "镜像 {} 失败 (连续: {} 次)",
                mirror, health.consecutive_failures
            );
        }
    }

    /// 检查镜像是否可用，超时后自动恢复
    pub fn is_available(&self, mirror: &str) -> bool {
        if let Some(mut health) = self.health.get_mut(mirror) {
            if let Some(disabled_at) = health.disabled_at {
                if disabled_at.elapsed() >= self.recovery_timeout {
                    debug!("镜像 {} 超时恢复", mirror);
                    health.disabled_at = None;
                    health.consecutive_failures = 0;
                    return true;
                }
                return false;
            }
            true
        } else {
            true
        }
    }

    /// 选择最佳可用镜像，优先选响应时间快的
    pub fn get_best_mirror(&self, mirrors: &[String]) -> Option<String> {
        let available: Vec<&String> = mirrors.iter().filter(|m| self.is_available(m)).collect();

        if available.is_empty() {
            return None;
        }

        if available.len() == 1 {
            return Some(available[0].clone());
        }

        available
            .iter()
            .min_by(|a, b| {
                let time_a = self
                    .health
                    .get(**a)
                    .map(|h| h.avg_response_time_ms)
                    .unwrap_or(f64::MAX);
                let time_b = self
                    .health
                    .get(**b)
                    .map(|h| h.avg_response_time_ms)
                    .unwrap_or(f64::MAX);
                time_a.partial_cmp(&time_b).unwrap_or(std::cmp::Ordering::Equal)
            })
            .cloned()
            .cloned()
    }

    /// 获取镜像统计信息
    pub fn get_stats(&self, mirror: &str) -> Option<MirrorStats> {
        self.health.get(mirror).map(|h| MirrorStats {
            mirror: mirror.to_string(),
            consecutive_failures: h.consecutive_failures,
            total_successes: h.total_successes,
            total_failures: h.total_failures,
            avg_response_time_ms: h.avg_response_time_ms,
            last_response_time_ms: h.last_response_time_ms,
            is_disabled: h.disabled_at.is_some(),
        })
    }

    /// 获取所有镜像的统计信息
    pub fn get_all_stats(&self) -> Vec<MirrorStats> {
        self.health
            .iter()
            .map(|entry| MirrorStats {
                mirror: entry.key().clone(),
                consecutive_failures: entry.value().consecutive_failures,
                total_successes: entry.value().total_successes,
                total_failures: entry.value().total_failures,
                avg_response_time_ms: entry.value().avg_response_time_ms,
                last_response_time_ms: entry.value().last_response_time_ms,
                is_disabled: entry.value().disabled_at.is_some(),
            })
            .collect()
    }
}

/// 镜像统计信息
#[derive(Debug, Clone)]
pub struct MirrorStats {
    pub mirror: String,
    pub consecutive_failures: u32,
    pub total_successes: u64,
    pub total_failures: u64,
    pub avg_response_time_ms: f64,
    pub last_response_time_ms: Option<f64>,
    pub is_disabled: bool,
}

