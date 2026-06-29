use serde::{Deserialize, Deserializer, Serialize};
use std::path::Path;

pub const DEFAULT_KV_CACHE_FREQUENT_POLL_SECS: u64 = 60;
pub const DEFAULT_KV_CACHE_LONG_POLL_SECS: u64 = 240;
pub const DEFAULT_KV_CACHE_CLAUDE_TTL_SECS: u64 = 3300;
pub const DEFAULT_KV_CACHE_OPENAI_TTL_SECS: u64 = 1700;
pub const DEFAULT_KV_CACHE_GLM_TTL_SECS: u64 = 540;
pub const DEFAULT_KV_CACHE_OTHER_TTL_SECS: u64 = 270;
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
pub struct ProviderTtls {
    #[serde(default = "default_kv_cache_claude_ttl_seconds")]
    pub claude: u64,
    #[serde(default = "default_kv_cache_openai_ttl_seconds")]
    pub openai: u64,
    #[serde(default = "default_kv_cache_glm_ttl_seconds")]
    pub glm: u64,
    #[serde(default = "default_kv_cache_other_ttl_seconds")]
    pub other: u64,
}

impl Default for ProviderTtls {
    fn default() -> Self {
        Self {
            claude: default_kv_cache_claude_ttl_seconds(),
            openai: default_kv_cache_openai_ttl_seconds(),
            glm: default_kv_cache_glm_ttl_seconds(),
            other: default_kv_cache_other_ttl_seconds(),
        }
    }
}

impl ProviderTtls {
    fn sanitized(mut self, path: Option<&Path>) -> Self {
        self.claude = sanitize_kv_cache_seconds(
            self.claude,
            "kv_cache.provider_ttls.claude",
            DEFAULT_KV_CACHE_CLAUDE_TTL_SECS,
            path,
        );
        self.openai = sanitize_kv_cache_seconds(
            self.openai,
            "kv_cache.provider_ttls.openai",
            DEFAULT_KV_CACHE_OPENAI_TTL_SECS,
            path,
        );
        self.glm = sanitize_kv_cache_seconds(
            self.glm,
            "kv_cache.provider_ttls.glm",
            DEFAULT_KV_CACHE_GLM_TTL_SECS,
            path,
        );
        self.other = sanitize_kv_cache_seconds(
            self.other,
            "kv_cache.provider_ttls.other",
            DEFAULT_KV_CACHE_OTHER_TTL_SECS,
            path,
        );
        self
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct KvCacheConfig {
    /// Poll interval for fast-changing external state such as GitHub bot events.
    #[serde(default = "default_kv_cache_frequent_poll_seconds")]
    pub frequent_poll_seconds: u64,
    /// Fallback TTL for `csa session wait` when the caller model provider is unknown.
    #[serde(default = "default_kv_cache_long_poll_seconds")]
    pub default_ttl_seconds: u64,
    /// Deprecated alias for `default_ttl_seconds`.
    ///
    /// Kept so old config readers and `csa config get kv_cache.long_poll_seconds`
    /// continue to see the effective fallback TTL.
    #[serde(default = "default_kv_cache_long_poll_seconds")]
    pub long_poll_seconds: u64,
    /// Provider-specific TTL caps for model-aware `csa session wait` calls.
    #[serde(default)]
    pub provider_ttls: ProviderTtls,
}

#[derive(Debug, Default, Deserialize)]
struct RawKvCacheConfig {
    #[serde(default)]
    frequent_poll_seconds: Option<u64>,
    #[serde(default)]
    default_ttl_seconds: Option<u64>,
    #[serde(default)]
    long_poll_seconds: Option<u64>,
    #[serde(default)]
    provider_ttls: ProviderTtls,
}

impl<'de> Deserialize<'de> for KvCacheConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawKvCacheConfig::deserialize(deserializer)?;
        let default_ttl_seconds = raw
            .default_ttl_seconds
            .or(raw.long_poll_seconds)
            .unwrap_or_else(default_kv_cache_long_poll_seconds);
        Ok(Self {
            frequent_poll_seconds: raw
                .frequent_poll_seconds
                .unwrap_or_else(default_kv_cache_frequent_poll_seconds),
            default_ttl_seconds,
            long_poll_seconds: default_ttl_seconds,
            provider_ttls: raw.provider_ttls,
        })
    }
}

fn default_kv_cache_frequent_poll_seconds() -> u64 {
    DEFAULT_KV_CACHE_FREQUENT_POLL_SECS
}

fn default_kv_cache_long_poll_seconds() -> u64 {
    DEFAULT_KV_CACHE_LONG_POLL_SECS
}

fn default_kv_cache_claude_ttl_seconds() -> u64 {
    DEFAULT_KV_CACHE_CLAUDE_TTL_SECS
}

fn default_kv_cache_openai_ttl_seconds() -> u64 {
    DEFAULT_KV_CACHE_OPENAI_TTL_SECS
}

fn default_kv_cache_glm_ttl_seconds() -> u64 {
    DEFAULT_KV_CACHE_GLM_TTL_SECS
}

fn default_kv_cache_other_ttl_seconds() -> u64 {
    DEFAULT_KV_CACHE_OTHER_TTL_SECS
}

impl Default for KvCacheConfig {
    fn default() -> Self {
        let default_ttl_seconds = default_kv_cache_long_poll_seconds();
        Self {
            frequent_poll_seconds: default_kv_cache_frequent_poll_seconds(),
            default_ttl_seconds,
            long_poll_seconds: default_ttl_seconds,
            provider_ttls: ProviderTtls::default(),
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
            self.default_ttl_seconds,
            "kv_cache.default_ttl_seconds",
            DEFAULT_KV_CACHE_LONG_POLL_SECS,
            path,
        );
        self.default_ttl_seconds = self.long_poll_seconds;
        self.provider_ttls = self.provider_ttls.sanitized(path);
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

#[cfg(test)]
mod tests {
    use super::{KvCacheConfig, ProviderTtls};

    #[test]
    fn kv_cache_defaults_include_provider_ttls() {
        let config = KvCacheConfig::default();

        assert_eq!(config.default_ttl_seconds, 240);
        assert_eq!(config.long_poll_seconds, 240);
        assert_eq!(config.provider_ttls.claude, 3300);
        assert_eq!(config.provider_ttls.openai, 1700);
        assert_eq!(config.provider_ttls.glm, 540);
        assert_eq!(config.provider_ttls.other, 270);
    }

    #[test]
    fn kv_cache_defaults_parse_when_section_omitted() {
        let config: KvCacheConfig = toml::from_str("").unwrap();

        assert_eq!(config.default_ttl_seconds, 240);
        assert_eq!(config.long_poll_seconds, 240);
        assert_eq!(config.provider_ttls, ProviderTtls::default());
    }

    #[test]
    fn legacy_long_poll_seconds_still_works() {
        let config: KvCacheConfig = toml::from_str(
            r#"
long_poll_seconds = 999
"#,
        )
        .unwrap();

        assert_eq!(config.default_ttl_seconds, 999);
        assert_eq!(config.long_poll_seconds, 999);
    }
}
