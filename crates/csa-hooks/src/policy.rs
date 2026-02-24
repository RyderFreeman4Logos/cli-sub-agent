use serde::{Deserialize, Serialize};

/// Hook failure handling strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailPolicy {
    /// Warn and continue when a hook fails.
    #[default]
    Open,
    /// Treat hook failure as a hard error unless a valid waiver exists.
    Closed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HookConfig;

    #[test]
    fn test_fail_policy_serde_roundtrip() {
        #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
        struct Wrapper {
            fail_policy: FailPolicy,
        }

        let encoded = toml::to_string(&Wrapper {
            fail_policy: FailPolicy::Closed,
        })
        .unwrap();
        let decoded: Wrapper = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded.fail_policy, FailPolicy::Closed);
    }

    #[test]
    fn test_fail_policy_default_is_open() {
        let config: HookConfig = toml::from_str(
            r#"
enabled = true
command = "echo test"
timeout_secs = 10
"#,
        )
        .unwrap();

        assert_eq!(config.fail_policy, FailPolicy::Open);
    }

    #[test]
    fn test_hook_config_deserializes_fail_policy() {
        let config: HookConfig = toml::from_str(
            r#"
enabled = true
command = "echo test"
timeout_secs = 10
fail_policy = "closed"
"#,
        )
        .unwrap();

        assert_eq!(config.fail_policy, FailPolicy::Closed);
    }
}
