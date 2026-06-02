use std::collections::HashMap;

use csa_core::env::{
    CSA_DEPTH_ENV_KEY, CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY, CSA_INTERNAL_INVOCATION_ENV_KEY,
    CSA_MODEL_SPEC_ENV_KEY, CSA_NO_FAILOVER_ENV_KEY, CSA_PARENT_SESSION_DIR_ENV_KEY,
    CSA_PARENT_SESSION_ENV_KEY, CSA_PROJECT_ROOT_ENV_KEY, CSA_SESSION_DIR_ENV_KEY,
    CSA_SESSION_ID_ENV_KEY, STARTUP_SUBTREE_ENV_KEYS,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StartupSubtreeEnv {
    session_id: Option<String>,
    depth: u32,
    project_root: Option<String>,
    session_dir: Option<String>,
    parent_session: Option<String>,
    parent_session_dir: Option<String>,
    internal_invocation: bool,
    model_spec: Option<String>,
    force_ignore_tier_setting: bool,
    no_failover: bool,
}

#[cfg(test)]
pub(crate) static EMPTY_STARTUP_SUBTREE_ENV: StartupSubtreeEnv = StartupSubtreeEnv {
    session_id: None,
    depth: 0,
    project_root: None,
    session_dir: None,
    parent_session: None,
    parent_session_dir: None,
    internal_invocation: false,
    model_spec: None,
    force_ignore_tier_setting: false,
    no_failover: false,
};

impl StartupSubtreeEnv {
    pub(crate) fn capture_and_remove_from_process_env() -> Self {
        let values = capture_startup_env_values(|key| std::env::var(key).ok());
        // SAFETY: this runs at the very start of CLI execution, before any
        // async work or child processes are started. Removing the frozen
        // subtree contract keys here prevents later env mutation/re-read.
        unsafe {
            for key in STARTUP_SUBTREE_ENV_KEYS {
                std::env::remove_var(key);
            }
        }
        Self::from_values(values)
    }

    pub(crate) fn from_values(values: HashMap<&'static str, String>) -> Self {
        let depth = values
            .get(CSA_DEPTH_ENV_KEY)
            .and_then(|raw| raw.parse::<u32>().ok())
            .unwrap_or(0);
        Self {
            session_id: non_empty(values.get(CSA_SESSION_ID_ENV_KEY)),
            depth,
            project_root: non_empty(values.get(CSA_PROJECT_ROOT_ENV_KEY)),
            session_dir: non_empty(values.get(CSA_SESSION_DIR_ENV_KEY)),
            parent_session: non_empty(values.get(CSA_PARENT_SESSION_ENV_KEY)),
            parent_session_dir: non_empty(values.get(CSA_PARENT_SESSION_DIR_ENV_KEY)),
            internal_invocation: values
                .get(CSA_INTERNAL_INVOCATION_ENV_KEY)
                .is_some_and(|value| is_truthy_env_value(value)),
            model_spec: non_empty(values.get(CSA_MODEL_SPEC_ENV_KEY)),
            force_ignore_tier_setting: values
                .get(CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY)
                .is_some_and(|value| is_truthy_env_value(value)),
            no_failover: values
                .get(CSA_NO_FAILOVER_ENV_KEY)
                .is_some_and(|value| is_truthy_env_value(value)),
        }
    }

    pub(crate) fn current_depth(&self) -> u32 {
        self.depth
    }

    pub(crate) fn next_depth_string(&self) -> String {
        self.depth.saturating_add(1).to_string()
    }

    pub(crate) fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub(crate) fn project_root(&self) -> Option<&str> {
        self.project_root.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn session_dir(&self) -> Option<&str> {
        self.session_dir.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn parent_session(&self) -> Option<&str> {
        self.parent_session.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn parent_session_dir(&self) -> Option<&str> {
        self.parent_session_dir.as_deref()
    }

    pub(crate) fn internal_invocation(&self) -> bool {
        self.internal_invocation
    }

    pub(crate) fn model_spec(&self) -> Option<&str> {
        self.model_spec.as_deref()
    }

    pub(crate) fn force_ignore_tier_setting(&self) -> bool {
        self.force_ignore_tier_setting
    }

    pub(crate) fn no_failover(&self) -> bool {
        self.no_failover
    }
}

fn capture_startup_env_values<F>(lookup: F) -> HashMap<&'static str, String>
where
    F: Fn(&str) -> Option<String>,
{
    STARTUP_SUBTREE_ENV_KEYS
        .iter()
        .filter_map(|&key| lookup(key).map(|value| (key, value)))
        .collect()
}

fn non_empty(value: Option<&String>) -> Option<String> {
    value
        .map(|raw| raw.trim())
        .filter(|raw| !raw.is_empty())
        .map(str::to_string)
}

pub(crate) fn is_truthy_env_value(raw: &str) -> bool {
    let normalized = raw.trim().to_ascii_lowercase();
    matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env_lock::ScopedEnvVarRestore;
    use serial_test::serial;

    #[test]
    fn startup_subtree_env_parses_values_without_process_env() {
        let values = HashMap::from([
            (
                CSA_SESSION_ID_ENV_KEY,
                "01KTESTSESSION0000000000".to_string(),
            ),
            (CSA_DEPTH_ENV_KEY, "3".to_string()),
            (CSA_PROJECT_ROOT_ENV_KEY, "/repo".to_string()),
            (CSA_SESSION_DIR_ENV_KEY, "/repo/.csa/session".to_string()),
            (
                CSA_PARENT_SESSION_ENV_KEY,
                "01KPARENTSESSION00000000".to_string(),
            ),
            (
                CSA_PARENT_SESSION_DIR_ENV_KEY,
                "/repo/.csa/parent".to_string(),
            ),
            (CSA_INTERNAL_INVOCATION_ENV_KEY, "yes".to_string()),
            (
                CSA_MODEL_SPEC_ENV_KEY,
                "codex/openai/gpt-5.5/xhigh".to_string(),
            ),
            (CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY, "1".to_string()),
            (CSA_NO_FAILOVER_ENV_KEY, "true".to_string()),
        ]);

        let startup = StartupSubtreeEnv::from_values(values);

        assert_eq!(startup.session_id(), Some("01KTESTSESSION0000000000"));
        assert_eq!(startup.current_depth(), 3);
        assert_eq!(startup.next_depth_string(), "4");
        assert_eq!(startup.project_root(), Some("/repo"));
        assert_eq!(startup.session_dir(), Some("/repo/.csa/session"));
        assert_eq!(startup.parent_session(), Some("01KPARENTSESSION00000000"));
        assert_eq!(startup.parent_session_dir(), Some("/repo/.csa/parent"));
        assert!(startup.internal_invocation());
        assert_eq!(startup.model_spec(), Some("codex/openai/gpt-5.5/xhigh"));
        assert!(startup.force_ignore_tier_setting());
        assert!(startup.no_failover());
    }

    #[test]
    #[serial]
    fn startup_capture_removes_subtree_keys_from_process_env() {
        let _depth = ScopedEnvVarRestore::set(CSA_DEPTH_ENV_KEY, "2");
        let _session = ScopedEnvVarRestore::set(CSA_SESSION_ID_ENV_KEY, "01KTESTSESSION");
        let _model = ScopedEnvVarRestore::set(CSA_MODEL_SPEC_ENV_KEY, "codex/openai/gpt-5/xhigh");
        let _force = ScopedEnvVarRestore::set(CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY, "1");

        let startup = StartupSubtreeEnv::capture_and_remove_from_process_env();

        assert_eq!(startup.current_depth(), 2);
        assert_eq!(startup.session_id(), Some("01KTESTSESSION"));
        assert_eq!(startup.model_spec(), Some("codex/openai/gpt-5/xhigh"));
        for key in STARTUP_SUBTREE_ENV_KEYS {
            assert!(
                std::env::var(key).is_err(),
                "{key} should be removed after startup capture"
            );
        }
    }
}
