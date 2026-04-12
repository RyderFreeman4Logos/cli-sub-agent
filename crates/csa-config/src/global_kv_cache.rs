use serde::{Deserialize, Serialize};
use std::path::Path;

pub const DEFAULT_KV_CACHE_FREQUENT_POLL_SECS: u64 = 60;
pub const DEFAULT_KV_CACHE_LONG_POLL_SECS: u64 = 240;
pub const LEGACY_SESSION_WAIT_FALLBACK_SECS: u64 = 250;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KvCacheValueSource {
    Configured,
    SectionDefault,
    DocumentedDefault,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedKvCacheValue {
    pub seconds: u64,
    pub source: KvCacheValueSource,
}

impl ResolvedKvCacheValue {
    pub(crate) fn documented_default() -> Self {
        Self {
            seconds: DEFAULT_KV_CACHE_LONG_POLL_SECS,
            source: KvCacheValueSource::DocumentedDefault,
        }
    }

    pub(crate) fn section_default() -> Self {
        Self {
            seconds: DEFAULT_KV_CACHE_LONG_POLL_SECS,
            source: KvCacheValueSource::SectionDefault,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KvCacheConfig {
    /// Poll interval for fast-changing external state such as GitHub bot events.
    #[serde(default = "default_kv_cache_frequent_poll_seconds")]
    pub frequent_poll_seconds: u64,
    /// Poll interval for long-running background waits that keep the caller KV cache warm.
    #[serde(default = "default_kv_cache_long_poll_seconds")]
    pub long_poll_seconds: u64,
}

fn default_kv_cache_frequent_poll_seconds() -> u64 {
    DEFAULT_KV_CACHE_FREQUENT_POLL_SECS
}

fn default_kv_cache_long_poll_seconds() -> u64 {
    DEFAULT_KV_CACHE_LONG_POLL_SECS
}

impl Default for KvCacheConfig {
    fn default() -> Self {
        Self {
            frequent_poll_seconds: default_kv_cache_frequent_poll_seconds(),
            long_poll_seconds: default_kv_cache_long_poll_seconds(),
        }
    }
}

impl KvCacheConfig {
    pub(crate) fn sanitized(mut self, path: Option<&Path>) -> Self {
        self.frequent_poll_seconds = sanitize_kv_cache_seconds(
            self.frequent_poll_seconds,
            "kv_cache.frequent_poll_seconds",
            DEFAULT_KV_CACHE_FREQUENT_POLL_SECS,
            path,
        );
        self.long_poll_seconds = sanitize_kv_cache_seconds(
            self.long_poll_seconds,
            "kv_cache.long_poll_seconds",
            DEFAULT_KV_CACHE_LONG_POLL_SECS,
            path,
        );
        self
    }
}

fn sanitize_kv_cache_seconds(value: u64, key: &str, default: u64, path: Option<&Path>) -> u64 {
    if value != 0 {
        return value;
    }

    match path {
        Some(path) => tracing::warn!(
            path = %path.display(),
            key,
            fallback = default,
            "Ignoring invalid zero-valued KV cache interval; using default"
        ),
        None => tracing::warn!(
            key,
            fallback = default,
            "Ignoring invalid zero-valued KV cache interval; using default"
        ),
    }
    default
}
