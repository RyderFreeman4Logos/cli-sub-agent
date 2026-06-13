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
fn pattern_internal_marker_is_captured_and_reemitted_to_children() {
    let startup = StartupSubtreeEnv::from_values(HashMap::from([
        (CSA_DEPTH_ENV_KEY, "1".to_string()),
        (CSA_PATTERN_INTERNAL_ENV_KEY, "1".to_string()),
    ]));

    assert!(startup.pattern_internal());
    assert!(
        startup
            .to_child_env_vars()
            .contains(&(CSA_PATTERN_INTERNAL_ENV_KEY.to_string(), "1".to_string())),
        "captured pattern-internal marker must propagate to the child env"
    );
}

#[test]
fn pattern_internal_marker_absent_is_not_emitted() {
    let startup =
        StartupSubtreeEnv::from_values(HashMap::from([(CSA_DEPTH_ENV_KEY, "0".to_string())]));

    assert!(!startup.pattern_internal());
    assert!(
        !startup
            .to_child_env_vars()
            .iter()
            .any(|(key, _)| key == CSA_PATTERN_INTERNAL_ENV_KEY)
    );
}

#[test]
fn pattern_internal_marker_survives_two_nested_csa_hops() {
    fn captured_from_parent(
        parent_child_env: &[(String, String)],
        depth: &str,
    ) -> HashMap<&'static str, String> {
        let mut values = HashMap::from([(CSA_DEPTH_ENV_KEY, depth.to_string())]);
        if parent_child_env
            .iter()
            .any(|(key, value)| key == CSA_PATTERN_INTERNAL_ENV_KEY && value == "1")
        {
            values.insert(CSA_PATTERN_INTERNAL_ENV_KEY, "1".to_string());
        }
        values
    }

    let depth1 = StartupSubtreeEnv::from_values(HashMap::from([
        (CSA_DEPTH_ENV_KEY, "1".to_string()),
        (CSA_PATTERN_INTERNAL_ENV_KEY, "1".to_string()),
    ]));
    let depth1_child_env = depth1.to_child_env_vars();
    assert!(
        depth1_child_env.contains(&(CSA_PATTERN_INTERNAL_ENV_KEY.to_string(), "1".to_string())),
        "depth-1 csa process must re-emit the marker to its child"
    );

    let depth2 = StartupSubtreeEnv::from_values(captured_from_parent(&depth1_child_env, "2"));
    assert_eq!(depth2.current_depth(), 2);
    assert!(
        depth2.pattern_internal(),
        "depth-2 csa debate must inherit the pattern-internal marker"
    );
    assert!(
        depth2
            .to_child_env_vars()
            .contains(&(CSA_PATTERN_INTERNAL_ENV_KEY.to_string(), "1".to_string())),
        "marker must keep propagating beyond depth 2"
    );
}

#[test]
fn startup_subtree_env_child_contract_env_reemits_identity_parent_and_trusted_pin() {
    let _lock = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let xdg = tempfile::tempdir().expect("xdg tempdir");
    let _xdg_guard = ScopedEnvVarRestore::set("XDG_STATE_HOME", xdg.path());
    let project = tempfile::tempdir().expect("project tempdir");
    let parent = csa_session::create_session(
        project.path(),
        Some("startup pin parent"),
        None,
        Some("codex"),
    )
    .expect("create parent session");
    let child = csa_session::create_session(
        project.path(),
        Some("startup pin child"),
        Some(&parent.meta_session_id),
        Some("codex"),
    )
    .expect("create child session");
    let child_dir =
        csa_session::get_session_dir(project.path(), &child.meta_session_id).expect("child dir");
    let parent_dir =
        csa_session::get_session_dir(project.path(), &parent.meta_session_id).expect("parent dir");
    let pin = crate::run_cmd_model_pin::resolve_subtree_model_pin(
        Some("codex/openai/gpt-5.5/xhigh"),
        true,
        true,
    )
    .expect("trusted pin");
    crate::run_cmd_model_pin::sync_subtree_model_pin_sidecar(
        project.path(),
        &child.meta_session_id,
        &child_dir,
        Some(&pin),
    )
    .expect("write trusted pin sidecar");

    let startup = StartupSubtreeEnv::from_values(HashMap::from([
        (CSA_SESSION_ID_ENV_KEY, child.meta_session_id.clone()),
        (CSA_SESSION_DIR_ENV_KEY, child_dir.display().to_string()),
        (CSA_PARENT_SESSION_ENV_KEY, parent.meta_session_id.clone()),
        (
            CSA_PARENT_SESSION_DIR_ENV_KEY,
            parent_dir.display().to_string(),
        ),
        (
            CSA_DEPTH_ENV_KEY,
            child.genealogy.depth.saturating_add(1).to_string(),
        ),
        (
            CSA_PROJECT_ROOT_ENV_KEY,
            project.path().display().to_string(),
        ),
        (CSA_INTERNAL_INVOCATION_ENV_KEY, "1".to_string()),
        (
            CSA_MODEL_SPEC_ENV_KEY,
            "codex/openai/gpt-5.5/xhigh".to_string(),
        ),
        (CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY, "1".to_string()),
        (CSA_NO_FAILOVER_ENV_KEY, "1".to_string()),
    ]));

    assert_eq!(
        startup.to_csa_child_contract_env_vars(),
        vec![
            (CSA_SESSION_ID_ENV_KEY.to_string(), child.meta_session_id),
            (
                CSA_SESSION_DIR_ENV_KEY.to_string(),
                child_dir.display().to_string(),
            ),
            (
                CSA_PARENT_SESSION_ENV_KEY.to_string(),
                parent.meta_session_id,
            ),
            (
                CSA_PARENT_SESSION_DIR_ENV_KEY.to_string(),
                parent_dir.display().to_string(),
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
    let startup = StartupSubtreeEnv::from_values(HashMap::from([
        (CSA_DEPTH_ENV_KEY, "0".to_string()),
        (
            CSA_MODEL_SPEC_ENV_KEY,
            "codex/openai/gpt-5.5/xhigh".to_string(),
        ),
        (CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY, "1".to_string()),
        (CSA_NO_FAILOVER_ENV_KEY, "1".to_string()),
    ]));

    assert!(startup.to_csa_child_contract_env_vars().is_empty());
}

#[test]
fn startup_subtree_env_with_current_session_updates_child_env() {
    let startup = StartupSubtreeEnv::from_values(HashMap::from([
        (CSA_SESSION_ID_ENV_KEY, "01KPARENT".to_string()),
        (CSA_SESSION_DIR_ENV_KEY, "/repo/parent".to_string()),
        (
            CSA_MODEL_SPEC_ENV_KEY,
            "codex/openai/gpt-5.5/xhigh".to_string(),
        ),
    ]))
    .with_current_session("01KCHILD", "/repo/child");

    assert_eq!(startup.session_id(), Some("01KCHILD"));
    assert_eq!(startup.session_dir(), Some("/repo/child"));
    assert_eq!(startup.parent_session(), Some("01KPARENT"));
    assert_eq!(startup.parent_session_dir(), Some("/repo/parent"));
    assert_eq!(
        startup.to_child_env_vars(),
        vec![
            (CSA_SESSION_ID_ENV_KEY.to_string(), "01KCHILD".to_string()),
            (
                CSA_SESSION_DIR_ENV_KEY.to_string(),
                "/repo/child".to_string(),
            ),
            (
                CSA_PARENT_SESSION_ENV_KEY.to_string(),
                "01KPARENT".to_string(),
            ),
            (
                CSA_PARENT_SESSION_DIR_ENV_KEY.to_string(),
                "/repo/parent".to_string(),
            ),
        ]
    );
}

#[test]
fn startup_subtree_env_with_current_session_does_not_self_parent() {
    let startup = StartupSubtreeEnv::from_values(HashMap::from([
        (CSA_SESSION_ID_ENV_KEY, "01KSESSION".to_string()),
        (CSA_SESSION_DIR_ENV_KEY, "/repo/session".to_string()),
    ]))
    .with_current_session("01KSESSION", "/repo/session");

    assert_eq!(startup.session_id(), Some("01KSESSION"));
    assert_eq!(startup.session_dir(), Some("/repo/session"));
    assert_eq!(startup.parent_session(), None);
    assert_eq!(
        startup.to_child_env_vars(),
        vec![
            (CSA_SESSION_ID_ENV_KEY.to_string(), "01KSESSION".to_string()),
            (
                CSA_SESSION_DIR_ENV_KEY.to_string(),
                "/repo/session".to_string(),
            ),
        ]
    );
}

#[test]
#[serial]
fn startup_capture_preserves_subtree_keys_in_process_env() {
    let _depth = ScopedEnvVarRestore::set(CSA_DEPTH_ENV_KEY, "2");
    let _session = ScopedEnvVarRestore::set(CSA_SESSION_ID_ENV_KEY, "01KTESTSESSION");
    let _model = ScopedEnvVarRestore::set(CSA_MODEL_SPEC_ENV_KEY, "codex/openai/gpt-5/xhigh");
    let _force = ScopedEnvVarRestore::set(CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY, "1");

    let startup = StartupSubtreeEnv::capture_from_process_env();

    assert_eq!(startup.current_depth(), 2);
    assert_eq!(startup.session_id(), Some("01KTESTSESSION"));
    assert_eq!(startup.model_spec(), Some("codex/openai/gpt-5/xhigh"));
    assert_eq!(std::env::var(CSA_DEPTH_ENV_KEY).as_deref(), Ok("2"));
    assert_eq!(
        std::env::var(CSA_SESSION_ID_ENV_KEY).as_deref(),
        Ok("01KTESTSESSION")
    );
    assert_eq!(
        std::env::var(CSA_MODEL_SPEC_ENV_KEY).as_deref(),
        Ok("codex/openai/gpt-5/xhigh")
    );
    assert_eq!(
        std::env::var(CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY).as_deref(),
        Ok("1")
    );
}
