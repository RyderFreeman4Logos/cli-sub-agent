use std::collections::HashMap;

use csa_core::env::{
    CSA_DEPTH_ENV_KEY, CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY, CSA_INTERNAL_INVOCATION_ENV_KEY,
    CSA_MODEL_SPEC_ENV_KEY, CSA_NO_FAILOVER_ENV_KEY, CSA_PARENT_SESSION_DIR_ENV_KEY,
    CSA_PARENT_SESSION_ENV_KEY, CSA_PROJECT_ROOT_ENV_KEY, CSA_SESSION_DIR_ENV_KEY,
    CSA_SESSION_ID_ENV_KEY, STARTUP_SUBTREE_ENV_KEYS,
};

const CSA_CHILD_CONTRACT_ENV_KEYS: &[&str] = &[
    CSA_SESSION_ID_ENV_KEY,
    CSA_SESSION_DIR_ENV_KEY,
    CSA_PARENT_SESSION_ENV_KEY,
    CSA_PARENT_SESSION_DIR_ENV_KEY,
    CSA_MODEL_SPEC_ENV_KEY,
    CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY,
    CSA_NO_FAILOVER_ENV_KEY,
];

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
    raw_session_id: Option<String>,
    raw_depth: Option<String>,
    raw_project_root: Option<String>,
    raw_session_dir: Option<String>,
    raw_parent_session: Option<String>,
    raw_parent_session_dir: Option<String>,
    raw_internal_invocation: Option<String>,
    raw_model_spec: Option<String>,
    raw_force_ignore_tier_setting: Option<String>,
    raw_no_failover: Option<String>,
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
    raw_session_id: None,
    raw_depth: None,
    raw_project_root: None,
    raw_session_dir: None,
    raw_parent_session: None,
    raw_parent_session_dir: None,
    raw_internal_invocation: None,
    raw_model_spec: None,
    raw_force_ignore_tier_setting: None,
    raw_no_failover: None,
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
        let raw_session_id = values.get(CSA_SESSION_ID_ENV_KEY).cloned();
        let raw_depth = values.get(CSA_DEPTH_ENV_KEY).cloned();
        let raw_project_root = values.get(CSA_PROJECT_ROOT_ENV_KEY).cloned();
        let raw_session_dir = values.get(CSA_SESSION_DIR_ENV_KEY).cloned();
        let raw_parent_session = values.get(CSA_PARENT_SESSION_ENV_KEY).cloned();
        let raw_parent_session_dir = values.get(CSA_PARENT_SESSION_DIR_ENV_KEY).cloned();
        let raw_internal_invocation = values.get(CSA_INTERNAL_INVOCATION_ENV_KEY).cloned();
        let raw_model_spec = values.get(CSA_MODEL_SPEC_ENV_KEY).cloned();
        let raw_force_ignore_tier_setting =
            values.get(CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY).cloned();
        let raw_no_failover = values.get(CSA_NO_FAILOVER_ENV_KEY).cloned();

        let depth = raw_depth
            .as_ref()
            .and_then(|raw| raw.parse::<u32>().ok())
            .unwrap_or(0);
        let internal_invocation = raw_internal_invocation
            .as_ref()
            .is_some_and(|value| is_truthy_env_value(value));
        let force_ignore_tier_setting = raw_force_ignore_tier_setting
            .as_ref()
            .is_some_and(|value| is_truthy_env_value(value));
        let no_failover = raw_no_failover
            .as_ref()
            .is_some_and(|value| is_truthy_env_value(value));

        Self {
            session_id: non_empty(raw_session_id.as_ref()),
            depth,
            project_root: non_empty(raw_project_root.as_ref()),
            session_dir: non_empty(raw_session_dir.as_ref()),
            parent_session: non_empty(raw_parent_session.as_ref()),
            parent_session_dir: non_empty(raw_parent_session_dir.as_ref()),
            internal_invocation,
            model_spec: non_empty(raw_model_spec.as_ref()),
            force_ignore_tier_setting,
            no_failover,
            raw_session_id,
            raw_depth,
            raw_project_root,
            raw_session_dir,
            raw_parent_session,
            raw_parent_session_dir,
            raw_internal_invocation,
            raw_model_spec,
            raw_force_ignore_tier_setting,
            raw_no_failover,
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

    pub(crate) fn with_current_session(
        mut self,
        session_id: impl AsRef<str>,
        session_dir: impl AsRef<str>,
    ) -> Self {
        self.session_id = non_empty_str(session_id.as_ref());
        self.raw_session_id = self.session_id.clone();
        self.session_dir = non_empty_str(session_dir.as_ref());
        self.raw_session_dir = self.session_dir.clone();
        self
    }

    pub(crate) fn apply_to_child_env(&self, env: &mut HashMap<String, String>) {
        for key in CSA_CHILD_CONTRACT_ENV_KEYS {
            env.remove(*key);
        }
        for (key, value) in self.to_child_env_vars() {
            env.insert(key, value);
        }
    }

    pub(crate) fn to_child_env_vars(&self) -> Vec<(String, String)> {
        let mut vars = self.to_csa_child_contract_env_vars();
        self.push_child_env_var(&mut vars, CSA_DEPTH_ENV_KEY, &self.raw_depth);
        self.push_child_env_var(&mut vars, CSA_PROJECT_ROOT_ENV_KEY, &self.raw_project_root);
        self.push_child_env_var(
            &mut vars,
            CSA_INTERNAL_INVOCATION_ENV_KEY,
            &self.raw_internal_invocation,
        );
        vars
    }

    pub(crate) fn csa_child_contract_env_keys() -> &'static [&'static str] {
        CSA_CHILD_CONTRACT_ENV_KEYS
    }

    pub(crate) fn to_csa_child_contract_env_vars(&self) -> Vec<(String, String)> {
        let mut vars = Vec::new();
        self.push_child_env_var(&mut vars, CSA_SESSION_ID_ENV_KEY, &self.raw_session_id);
        self.push_child_env_var(&mut vars, CSA_SESSION_DIR_ENV_KEY, &self.raw_session_dir);
        self.push_child_env_var(
            &mut vars,
            CSA_PARENT_SESSION_ENV_KEY,
            &self.raw_parent_session,
        );
        self.push_child_env_var(
            &mut vars,
            CSA_PARENT_SESSION_DIR_ENV_KEY,
            &self.raw_parent_session_dir,
        );
        let inherited_model_pin = crate::run_cmd_model_pin::inherited_model_pin_from_startup(self);
        if let Some(pin) =
            crate::run_cmd_model_pin::inherited_subtree_model_pin(inherited_model_pin.as_ref())
        {
            for (key, value) in pin.pin_env_entries() {
                vars.push((key.to_string(), value));
            }
        }
        vars
    }

    fn push_child_env_var(
        &self,
        vars: &mut Vec<(String, String)>,
        key: &'static str,
        value: &Option<String>,
    ) {
        if let Some(value) = value {
            vars.push((key.to_string(), value.clone()));
        }
    }

    pub(crate) fn project_root(&self) -> Option<&str> {
        self.project_root.as_deref()
    }

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
    value.and_then(|raw| non_empty_str(raw))
}

fn non_empty_str(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
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
    fn startup_subtree_env_reemits_only_captured_keys() {
        let values = HashMap::from([
            (CSA_DEPTH_ENV_KEY, "0".to_string()),
            (CSA_PROJECT_ROOT_ENV_KEY, "/repo".to_string()),
            (CSA_INTERNAL_INVOCATION_ENV_KEY, "1".to_string()),
            (CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY, "false".to_string()),
            (CSA_NO_FAILOVER_ENV_KEY, "0".to_string()),
        ]);

        let startup = StartupSubtreeEnv::from_values(values);
        let child_env = startup.to_child_env_vars();

        assert_eq!(
            child_env,
            vec![
                (CSA_DEPTH_ENV_KEY.to_string(), "0".to_string()),
                (CSA_PROJECT_ROOT_ENV_KEY.to_string(), "/repo".to_string()),
                (CSA_INTERNAL_INVOCATION_ENV_KEY.to_string(), "1".to_string()),
            ]
        );
        assert!(!startup.force_ignore_tier_setting());
        assert!(!startup.no_failover());
    }

    #[test]
    fn startup_subtree_env_child_contract_env_reemits_identity_parent_and_trusted_pin() {
        let values = HashMap::from([
            (CSA_SESSION_ID_ENV_KEY, "01KSESSION".to_string()),
            (CSA_SESSION_DIR_ENV_KEY, "/repo/session".to_string()),
            (CSA_PARENT_SESSION_ENV_KEY, "01KPARENT".to_string()),
            (CSA_PARENT_SESSION_DIR_ENV_KEY, "/repo/parent".to_string()),
            (CSA_DEPTH_ENV_KEY, "2".to_string()),
            (
                CSA_MODEL_SPEC_ENV_KEY,
                "codex/openai/gpt-5.5/xhigh".to_string(),
            ),
            (CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY, "1".to_string()),
            (CSA_NO_FAILOVER_ENV_KEY, "1".to_string()),
        ]);

        let startup = StartupSubtreeEnv::from_values(values);
        let child_contract_env = startup.to_csa_child_contract_env_vars();

        assert_eq!(
            child_contract_env,
            vec![
                (CSA_SESSION_ID_ENV_KEY.to_string(), "01KSESSION".to_string()),
                (
                    CSA_SESSION_DIR_ENV_KEY.to_string(),
                    "/repo/session".to_string()
                ),
                (
                    CSA_PARENT_SESSION_ENV_KEY.to_string(),
                    "01KPARENT".to_string()
                ),
                (
                    CSA_PARENT_SESSION_DIR_ENV_KEY.to_string(),
                    "/repo/parent".to_string()
                ),
                (
                    CSA_MODEL_SPEC_ENV_KEY.to_string(),
                    "codex/openai/gpt-5.5/xhigh".to_string(),
                ),
                (
                    CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY.to_string(),
                    "1".to_string(),
                ),
                (CSA_NO_FAILOVER_ENV_KEY.to_string(), "1".to_string()),
            ]
        );
    }

    #[test]
    fn startup_subtree_env_child_contract_env_does_not_emit_root_pin() {
        let values = HashMap::from([
            (CSA_DEPTH_ENV_KEY, "0".to_string()),
            (
                CSA_MODEL_SPEC_ENV_KEY,
                "codex/openai/gpt-5.5/xhigh".to_string(),
            ),
            (CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY, "1".to_string()),
            (CSA_NO_FAILOVER_ENV_KEY, "1".to_string()),
        ]);

        let startup = StartupSubtreeEnv::from_values(values);
        let child_contract_env = startup.to_csa_child_contract_env_vars();

        assert!(child_contract_env.is_empty());
    }

    #[test]
    fn startup_subtree_env_with_current_session_updates_child_env() {
        let values = HashMap::from([
            (CSA_SESSION_ID_ENV_KEY, "01KPARENT".to_string()),
            (CSA_SESSION_DIR_ENV_KEY, "/repo/parent".to_string()),
            (
                CSA_MODEL_SPEC_ENV_KEY,
                "codex/openai/gpt-5.5/xhigh".to_string(),
            ),
        ]);

        let startup =
            StartupSubtreeEnv::from_values(values).with_current_session("01KCHILD", "/repo/child");
        let child_env = startup.to_child_env_vars();

        assert_eq!(startup.session_id(), Some("01KCHILD"));
        assert_eq!(startup.session_dir(), Some("/repo/child"));
        assert_eq!(
            child_env,
            vec![
                (CSA_SESSION_ID_ENV_KEY.to_string(), "01KCHILD".to_string()),
                (
                    CSA_SESSION_DIR_ENV_KEY.to_string(),
                    "/repo/child".to_string()
                ),
            ]
        );
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
