use std::path::{Path, PathBuf};

use crate::global::{DEFAULT_KV_CACHE_LONG_POLL_SECS, KvCacheConfig};

const HERMES_MODEL_PROVIDER_ENV: &str = "HERMES_MODEL_PROVIDER";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelProvider {
    Claude,
    OpenAI,
    Glm,
    Other,
}

impl ModelProvider {
    fn from_hermes_value(value: &str) -> Option<Self> {
        let normalized = normalize_provider_name(value);
        if normalized.is_empty() {
            return None;
        }
        Some(match normalized.as_str() {
            "anthropic" | "claude" => Self::Claude,
            "openai" => Self::OpenAI,
            "zai" | "zhipu" | "zhipuai" | "glm" => Self::Glm,
            _ => Self::Other,
        })
    }
}

pub fn parse_model_provider(value: &str) -> Result<ModelProvider, String> {
    let normalized = normalize_provider_name(value);
    match normalized.as_str() {
        "anthropic" | "claude" => Ok(ModelProvider::Claude),
        "openai" => Ok(ModelProvider::OpenAI),
        "zai" | "zhipu" | "zhipuai" | "glm" => Ok(ModelProvider::Glm),
        "other" => Ok(ModelProvider::Other),
        _ => Err(format!(
            "unsupported model provider '{value}'; expected claude, openai, glm, or other"
        )),
    }
}

pub fn detect_model_provider() -> Option<ModelProvider> {
    detect_model_provider_from_env().or_else(detect_model_provider_from_hermes_config)
}

/// Maximum allowed wait TTL in seconds (#2538).
/// Provider TTLs exceeding this cap are clamped to prevent excessively long waits.
pub const MAX_WAIT_TTL_SECONDS: u64 = 3000;

pub fn provider_ttl(provider: ModelProvider, config: &KvCacheConfig) -> u64 {
    let provider_seconds = match provider {
        ModelProvider::Claude => config.provider_ttls.claude,
        ModelProvider::OpenAI => config.provider_ttls.openai,
        ModelProvider::Glm => config.provider_ttls.glm,
        ModelProvider::Other => config.provider_ttls.other,
    };
    let resolved = if provider_seconds > 0 {
        provider_seconds
    } else {
        fallback_default_ttl(config)
    };
    resolved.min(MAX_WAIT_TTL_SECONDS)
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

fn fallback_default_ttl(config: &KvCacheConfig) -> u64 {
    if config.default_ttl_seconds > 0 {
        config.default_ttl_seconds
    } else if config.long_poll_seconds > 0 {
        config.long_poll_seconds
    } else {
        DEFAULT_KV_CACHE_LONG_POLL_SECS
    }
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

        assert_eq!(detect_model_provider(), Some(ModelProvider::Glm));
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

        assert_eq!(detect_model_provider(), Some(ModelProvider::Claude));
    }

    #[test]
    fn provider_ttl_uses_correct_configured_value() {
        let mut config = KvCacheConfig::default();
        config.provider_ttls.openai = 1800;

        assert_eq!(provider_ttl(ModelProvider::OpenAI, &config), 1800);
    }

    #[test]
    fn provider_ttl_falls_back_to_default() {
        let config = KvCacheConfig {
            default_ttl_seconds: 123,
            provider_ttls: crate::ProviderTtls {
                glm: 0,
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(provider_ttl(ModelProvider::Glm, &config), 123);
    }

    #[test]
    fn issue_2538_provider_ttl_clamped_to_max_cap() {
        let config = KvCacheConfig {
            default_ttl_seconds: 540,
            long_poll_seconds: 540,
            frequent_poll_seconds: 30,
            provider_ttls: crate::ProviderTtls {
                claude: 5000,
                openai: 0,
                glm: 0,
                other: 0,
            },
        };
        // Claude TTL 5000 > 3000 cap → clamped to 3000
        assert_eq!(provider_ttl(ModelProvider::Claude, &config), 3000);
        // OpenAI falls back to default 540 < 3000 → not clamped
        assert_eq!(provider_ttl(ModelProvider::OpenAI, &config), 540);
    }
}
