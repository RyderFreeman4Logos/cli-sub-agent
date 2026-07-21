use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
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

/// Explicitly configured provider TTL values keyed by normalized provider name.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ProviderTtls(pub BTreeMap<String, u64>);

impl ProviderTtls {
    fn sanitized(self, path: Option<&Path>) -> Self {
        for (provider, seconds) in &self.0 {
            if *seconds == 0 {
                let key = format!("kv_cache.provider_ttls.{provider}");
                match path {
                    Some(path) => tracing::warn!(
                        path = %path.display(),
                        key,
                        "Provider TTL is zero; csa session wait will reject this provider"
                    ),
                    None => tracing::warn!(
                        key,
                        "Provider TTL is zero; csa session wait will reject this provider"
                    ),
                }
            }
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct KvCacheConfig {
    /// Poll interval for fast-changing external state such as GitHub bot events.
    #[serde(default = "default_kv_cache_frequent_poll_seconds")]
    pub frequent_poll_seconds: u64,
    /// General long-poll TTL. `csa session wait` requires a provider-specific TTL.
    #[serde(default = "default_kv_cache_long_poll_seconds")]
    pub default_ttl_seconds: u64,
    /// Deprecated alias for `default_ttl_seconds`.
    ///
    /// Kept so old config readers and `csa config get kv_cache.long_poll_seconds`
    /// continue to see the effective general long-poll TTL.
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
    use std::collections::BTreeMap;

    #[test]
    fn kv_cache_defaults_have_no_explicit_provider_ttls() {
        let config = KvCacheConfig::default();

        assert_eq!(config.default_ttl_seconds, 240);
        assert_eq!(config.long_poll_seconds, 240);
        assert!(config.provider_ttls.0.is_empty());
    }

    #[test]
    fn kv_cache_defaults_parse_when_section_omitted() {
        let config: KvCacheConfig = toml::from_str("").unwrap();

        assert_eq!(config.default_ttl_seconds, 240);
        assert_eq!(config.long_poll_seconds, 240);
        assert_eq!(config.provider_ttls, ProviderTtls::default());
    }

    #[test]
    fn provider_ttls_parse_only_explicit_keys() {
        let config: KvCacheConfig = toml::from_str(
            r#"
[provider_ttls]
openai = 1800
gemini = 900
"#,
        )
        .unwrap();

        assert_eq!(config.provider_ttls.0.len(), 2);
        assert_eq!(config.provider_ttls.0["openai"], 1800);
        assert_eq!(config.provider_ttls.0["gemini"], 900);
        assert!(!config.provider_ttls.0.contains_key("claude"));
        assert!(!config.provider_ttls.0.contains_key("xai"));
    }

    #[test]
    fn provider_ttls_keep_zero_values_invalid_for_strict_wait_lookup() {
        let provider_ttls = ProviderTtls(BTreeMap::from([
            ("claude".to_string(), 0),
            ("custom".to_string(), 0),
        ]));

        let provider_ttls = provider_ttls.sanitized(None);

        assert_eq!(provider_ttls.0["claude"], 0);
        assert_eq!(provider_ttls.0["custom"], 0);
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
