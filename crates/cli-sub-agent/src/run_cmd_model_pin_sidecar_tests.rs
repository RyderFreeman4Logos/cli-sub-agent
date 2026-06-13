use super::*;

#[test]
fn csa_injected_pin_still_propagates() {
    let _lock = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let xdg = tempfile::tempdir().expect("xdg tempdir");
    let _xdg_guard = crate::test_env_lock::ScopedEnvVarRestore::set("XDG_STATE_HOME", xdg.path());
    let project = tempfile::tempdir().expect("project tempdir");
    let startup_env = trusted_startup_env_for_pinned_session(project.path(), PINNED_SPEC, true);

    let pin = inherited_model_pin_from_startup(&startup_env).expect("CSA-injected pin is honored");
    assert_eq!(pin.model_spec, PINNED_SPEC);
    assert!(pin.force_ignore_tier_setting);
    assert!(pin.no_failover);
}

#[test]
fn complete_ambient_env_spoof_without_sidecar_is_not_inherited() {
    let startup_env = StartupSubtreeEnv::from_values(std::collections::HashMap::from([
        (csa_core::env::CSA_DEPTH_ENV_KEY, "1".to_string()),
        (CSA_INTERNAL_INVOCATION_ENV_KEY, "1".to_string()),
        (CSA_SESSION_ID_ENV_KEY, TEST_SESSION_ID.to_string()),
        (CSA_SESSION_DIR_ENV_KEY, TEST_SESSION_DIR.to_string()),
        (CSA_PROJECT_ROOT_ENV_KEY, TEST_PROJECT_ROOT.to_string()),
        (CSA_MODEL_SPEC_ENV_KEY, PINNED_SPEC.to_string()),
        (CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY, "1".to_string()),
        (CSA_NO_FAILOVER_ENV_KEY, "1".to_string()),
    ]));

    assert!(
        inherited_model_pin_from_startup(&startup_env).is_none(),
        "complete ambient CSA_* values without CSA-owned sidecar must not bypass tier policy"
    );
}

#[test]
fn inherited_pin_requires_sidecar_session_contract_match() {
    let _lock = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let xdg = tempfile::tempdir().expect("xdg tempdir");
    let _xdg_guard = crate::test_env_lock::ScopedEnvVarRestore::set("XDG_STATE_HOME", xdg.path());
    let project = tempfile::tempdir().expect("project tempdir");
    let session =
        csa_session::create_session(project.path(), Some("pinned subtree"), None, Some("codex"))
            .expect("create pinned session");
    let session_dir = csa_session::get_session_dir(project.path(), &session.meta_session_id)
        .expect("session dir");
    let pin = resolve_subtree_model_pin(Some(PINNED_SPEC), true, true).expect("typed pin");
    sync_subtree_model_pin_sidecar(
        project.path(),
        "01KWRONGSESSION0000000000",
        &session_dir,
        Some(&pin),
    )
    .expect("write mismatched sidecar");
    let startup_env = StartupSubtreeEnv::from_values(std::collections::HashMap::from([
        (
            csa_core::env::CSA_DEPTH_ENV_KEY,
            session.genealogy.depth.saturating_add(1).to_string(),
        ),
        (CSA_INTERNAL_INVOCATION_ENV_KEY, "1".to_string()),
        (CSA_SESSION_ID_ENV_KEY, session.meta_session_id),
        (CSA_SESSION_DIR_ENV_KEY, session_dir.display().to_string()),
        (
            CSA_PROJECT_ROOT_ENV_KEY,
            project.path().display().to_string(),
        ),
        (CSA_MODEL_SPEC_ENV_KEY, PINNED_SPEC.to_string()),
        (CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY, "1".to_string()),
        (CSA_NO_FAILOVER_ENV_KEY, "1".to_string()),
    ]));

    assert!(
        inherited_model_pin_from_startup(&startup_env).is_none(),
        "sidecar identity must match the startup session contract before a pin is trusted"
    );
}
