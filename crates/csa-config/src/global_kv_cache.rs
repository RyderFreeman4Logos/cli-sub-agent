use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub const DEFAULT_KV_CACHE_FREQUENT_POLL_SECS: u64 = 60;
pub const DEFAULT_KV_CACHE_LONG_POLL_SECS: u64 = 240;
pub const DEFAULT_KV_CACHE_CLAUDE_TTL_SECS: u64 = 3300;
pub const DEFAULT_KV_CACHE_OPENAI_TTL_SECS: u64 = 1700;
pub const DEFAULT_KV_CACHE_GLM_TTL_SECS: u64 = 540;
pub const DEFAULT_KV_CACHE_XAI_TTL_SECS: u64 = 1700;
pub const DEFAULT_KV_CACHE_OTHER_TTL_SECS: u64 = 270;
pub const LEGACY_SESSION_WAIT_FALLBACK_SECS: u64 = 250;

const DEFAULT_PROVIDER_TTLS: [(&str, u64); 5] = [
    ("claude", DEFAULT_KV_CACHE_CLAUDE_TTL_SECS),
    ("openai", DEFAULT_KV_CACHE_OPENAI_TTL_SECS),
    ("glm", DEFAULT_KV_CACHE_GLM_TTL_SECS),
    ("xai", DEFAULT_KV_CACHE_XAI_TTL_SECS),
    ("other", DEFAULT_KV_CACHE_OTHER_TTL_SECS),
];

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

/// Provider-specific TTL values keyed by normalized provider name.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ProviderTtls(pub BTreeMap<String, u64>);

impl Default for ProviderTtls {
    fn default() -> Self {
        Self(default_provider_ttls())
    }
}

impl<'de> Deserialize<'de> for ProviderTtls {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let overrides = BTreeMap::<String, u64>::deserialize(deserializer)?;
        let mut provider_ttls = default_provider_ttls();
        provider_ttls.extend(overrides);
        Ok(Self(provider_ttls))
    }
}

impl ProviderTtls {
    fn sanitized(mut self, path: Option<&Path>) -> Self {
        for (provider, default) in DEFAULT_PROVIDER_TTLS {
            let entry = self.0.entry(provider.to_string()).or_insert(default);
            if *entry == 0 {
                let key = format!("kv_cache.provider_ttls.{provider}");
                *entry = sanitize_kv_cache_seconds(*entry, &key, default, path);
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

fn default_provider_ttls() -> BTreeMap<String, u64> {
    DEFAULT_PROVIDER_TTLS
        .into_iter()
        .map(|(provider, seconds)| (provider.to_string(), seconds))
        .collect()
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
    fn kv_cache_defaults_include_provider_ttls() {
        let config = KvCacheConfig::default();

        assert_eq!(config.default_ttl_seconds, 240);
        assert_eq!(config.long_poll_seconds, 240);
        assert_eq!(config.provider_ttls.0["claude"], 3300);
        assert_eq!(config.provider_ttls.0["openai"], 1700);
        assert_eq!(config.provider_ttls.0["glm"], 540);
        assert_eq!(config.provider_ttls.0["xai"], 1700);
        assert_eq!(config.provider_ttls.0["other"], 270);
    }

    #[test]
    fn kv_cache_defaults_parse_when_section_omitted() {
        let config: KvCacheConfig = toml::from_str("").unwrap();

        assert_eq!(config.default_ttl_seconds, 240);
        assert_eq!(config.long_poll_seconds, 240);
        assert_eq!(config.provider_ttls, ProviderTtls::default());
    }

    #[test]
    fn provider_ttls_parse_defaults_and_custom_keys() {
        let config: KvCacheConfig = toml::from_str(
            r#"
[provider_ttls]
openai = 1800
gemini = 900
"#,
        )
        .unwrap();

        assert_eq!(config.provider_ttls.0["claude"], 3300);
        assert_eq!(config.provider_ttls.0["openai"], 1800);
        assert_eq!(config.provider_ttls.0["xai"], 1700);
        assert_eq!(config.provider_ttls.0["gemini"], 900);
    }

    #[test]
    fn provider_ttls_sanitize_zero_known_defaults_and_keep_custom() {
        let provider_ttls = ProviderTtls(BTreeMap::from([
            ("claude".to_string(), 0),
            ("custom".to_string(), 0),
        ]));

        let provider_ttls = provider_ttls.sanitized(None);

        assert_eq!(provider_ttls.0["claude"], 3300);
        assert_eq!(provider_ttls.0["custom"], 0);
        assert_eq!(provider_ttls.0["xai"], 1700);
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
