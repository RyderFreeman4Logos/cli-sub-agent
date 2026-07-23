use std::path::{Path, PathBuf};

use crate::global::KvCacheConfig;

const HERMES_MODEL_PROVIDER_ENV: &str = "HERMES_MODEL_PROVIDER";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelProvider(String);

impl ModelProvider {
    pub fn new(name: &str) -> Self {
        Self(normalize_provider_name(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn from_hermes_value(value: &str) -> Option<Self> {
        let normalized = normalize_provider_name(value);
        if normalized.is_empty() {
            return None;
        }
        Some(match normalized.as_str() {
            "anthropic" | "claude" => Self::new("claude"),
            "openai" | "openai-codex" => Self::new("openai"),
            "zai" | "zhipu" | "zhipuai" | "glm" => Self::new("glm"),
            "xai" | "xai-oauth" | "grok" => Self::new("xai"),
            _ => Self(normalized),
        })
    }
}

pub fn parse_model_provider(value: &str) -> Result<ModelProvider, String> {
    let normalized = normalize_provider_name(value);
    if normalized.is_empty() {
        Err(format!(
            "unsupported model provider '{value}'; expected a non-empty provider name"
        ))
    } else {
        Ok(ModelProvider(normalized))
    }
}

pub fn detect_model_provider() -> Option<ModelProvider> {
    detect_model_provider_from_env().or_else(detect_model_provider_from_hermes_config)
}

/// Return the provider-specific session-wait TTL exactly as configured.
///
/// A missing or zero value is invalid for session waits; callers must report a
/// fail-closed diagnostic rather than substitute a generic TTL.
pub fn provider_ttl(provider: &ModelProvider, config: &KvCacheConfig) -> Option<u64> {
    config
        .provider_ttls
        .0
        .get(provider.as_str())
        .copied()
        .filter(|seconds| *seconds > 0)
}

fn detect_model_provider_from_env() -> Option<ModelProvider> {
    std::env::var(HERMES_MODEL_PROVIDER_ENV)
        .ok()
        .and_then(|value| ModelProvider::from_hermes_value(&value))
}

fn detect_model_provider_from_hermes_config() -> Option<ModelProvider> {
    detect_model_provider_from_hermes_config_path(&default_hermes_config_path()?)
}

fn default_hermes_config_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".hermes/config.yaml"))
}

fn detect_model_provider_from_hermes_config_path(path: &Path) -> Option<ModelProvider> {
    let content = std::fs::read_to_string(path).ok()?;
    parse_model_provider_from_hermes_config(&content)
}

fn parse_model_provider_from_hermes_config(content: &str) -> Option<ModelProvider> {
    let mut model_indent = None;

    for raw_line in content.lines() {
        let line = raw_line.trim_end();
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let indent = line.len().saturating_sub(trimmed.len());
        if model_indent.is_some_and(|base| indent <= base) {
            model_indent = None;
        }

        let Some((key, value)) = yaml_key_value(trimmed) else {
            continue;
        };
        if key == "model.provider" {
            return ModelProvider::from_hermes_value(value);
        }
        if key == "model" && value.is_empty() {
            model_indent = Some(indent);
            continue;
        }
        if model_indent.is_some() && key == "provider" {
            return ModelProvider::from_hermes_value(value);
        }
    }

    None
}

fn yaml_key_value(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once(':')?;
    Some((key.trim(), trim_yaml_scalar(value)))
}

fn trim_yaml_scalar(value: &str) -> &str {
    value
        .split('#')
        .next()
        .unwrap_or(value)
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
}

fn normalize_provider_name(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    struct EnvVarGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let original = std::env::var(key).ok();
            // SAFETY: test-scoped env mutation is reverted in Drop.
            unsafe { std::env::set_var(key, value) };
            Self { key, original }
        }

        fn remove(key: &'static str) -> Self {
            let original = std::env::var(key).ok();
            // SAFETY: test-scoped env mutation is reverted in Drop.
            unsafe { std::env::remove_var(key) };
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: test-scoped env mutation is reverted in Drop.
            unsafe {
                match self.original.as_deref() {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    #[test]
    #[serial]
    fn detect_model_provider_from_env() {
        let _guard = EnvVarGuard::set(HERMES_MODEL_PROVIDER_ENV, "zhipuai");

        assert_eq!(detect_model_provider(), Some(ModelProvider::new("glm")));
    }

    #[test]
    #[serial]
    fn detect_model_provider_from_hermes_config() {
        let _provider_guard = EnvVarGuard::remove(HERMES_MODEL_PROVIDER_ENV);
        let home = tempfile::tempdir().unwrap();
        let _home_guard = EnvVarGuard::set("HOME", home.path());
        let hermes_dir = home.path().join(".hermes");
        std::fs::create_dir_all(&hermes_dir).unwrap();
        std::fs::write(
            hermes_dir.join("config.yaml"),
            r#"
model:
  provider: anthropic
"#,
        )
        .unwrap();

        assert_eq!(detect_model_provider(), Some(ModelProvider::new("claude")));
    }

    #[test]
    fn provider_ttl_uses_correct_configured_value() {
        let mut config = KvCacheConfig::default();
        config.provider_ttls.0.insert("openai".to_string(), 1800);

        assert_eq!(
            provider_ttl(&ModelProvider::new("openai"), &config),
            Some(1800)
        );
    }

    #[test]
    fn from_hermes_value_detects_xai_oauth() {
        assert_eq!(
            ModelProvider::from_hermes_value("xai-oauth"),
            Some(ModelProvider::new("xai"))
        );
    }

    #[test]
    fn from_hermes_value_maps_openai_codex_to_openai() {
        assert_eq!(
            ModelProvider::from_hermes_value("openai-codex"),
            Some(ModelProvider::new("openai"))
        );
    }

    #[test]
    fn from_hermes_value_preserves_unknown_provider() {
        assert_eq!(
            ModelProvider::from_hermes_value("gemini"),
            Some(ModelProvider::new("gemini"))
        );
    }

    #[test]
    fn parse_model_provider_accepts_custom_provider() {
        assert_eq!(
            parse_model_provider("gemini"),
            Ok(ModelProvider::new("gemini"))
        );
    }

    #[test]
    fn provider_ttl_rejects_zero_ttl() {
        let mut provider_ttls = crate::ProviderTtls::default();
        provider_ttls.0.insert("glm".to_string(), 0);
        let config = KvCacheConfig {
            default_ttl_seconds: 123,
            provider_ttls,
            ..Default::default()
        };

        assert_eq!(provider_ttl(&ModelProvider::new("glm"), &config), None);
    }

    #[test]
    fn provider_ttl_rejects_missing_custom_provider() {
        let config = KvCacheConfig {
            default_ttl_seconds: 123,
            ..Default::default()
        };

        assert_eq!(provider_ttl(&ModelProvider::new("gemini"), &config), None);
    }

    #[test]
    fn provider_ttl_honors_exact_configured_value_above_3000() {
        let provider_ttls = crate::ProviderTtls(std::collections::BTreeMap::from([
            ("xai".to_string(), 3300),
            ("openai".to_string(), 0),
        ]));
        let config = KvCacheConfig {
            default_ttl_seconds: 540,
            long_poll_seconds: 540,
            frequent_poll_seconds: 30,
            provider_ttls,
        };

        assert_eq!(
            provider_ttl(&ModelProvider::new("xai"), &config),
            Some(3300)
        );
        // Zero-valued providers remain invalid rather than using the general TTL.
        assert_eq!(provider_ttl(&ModelProvider::new("openai"), &config), None);
    }
}
