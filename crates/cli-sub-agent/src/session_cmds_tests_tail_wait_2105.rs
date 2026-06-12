use super::*;
use crate::session_cmds_daemon::{
    WaitBehavior, WaitLoopTiming, WaitReconciliationOutcome, handle_session_wait_with_hooks,
};
use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
use tempfile::tempdir;

#[test]
fn handle_session_wait_uses_persisted_current_turn_artifact_after_multi_turn_state_save() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = ScopedEnvVarRestore::set("HOME", td.path());
    let _state_guard = ScopedEnvVarRestore::set("XDG_STATE_HOME", &state_home);
    let _contract_guard = ScopedEnvVarRestore::unset(csa_session::RESULT_TOML_PATH_CONTRACT_ENV);
    let project = td.path();

    let mut session = create_session(
        project,
        Some("wait-current-turn-marker-after-multiturn-save"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_id = session.meta_session_id.clone();
    let session_dir = get_session_dir(project, &session_id).expect("session dir");
    let current_turn_artifact = csa_session::turn_contract_result_artifact_path(1);
    let current_turn_path = session_dir.join(&current_turn_artifact);
    std::fs::create_dir_all(current_turn_path.parent().expect("turn result parent"))
        .expect("create turn result dir");
    let current_turn_result = SessionResult {
        summary: "current invocation result from pre-run turn path".to_string(),
        ..make_result("success", 0)
    };
    let current_turn_result_toml =
        toml::to_string_pretty(&current_turn_result).expect("serialize current turn result");
    std::fs::write(&current_turn_path, current_turn_result_toml)
        .expect("write current turn result");
    let marker_contents = format!("artifact_path = \"{current_turn_artifact}\"\n");
    std::fs::write(
        crate::pipeline::result_contract::current_result_artifact_marker_path(&session_dir),
        marker_contents,
    )
    .expect("write current result artifact marker");
    session.turn_count = 3;
    save_session(&session).expect("save post-run multi-turn count");
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .expect("write daemon completion");
    assert!(
        !csa_session::turn_contract_result_path(&session_dir, 4).exists(),
        "test setup requires saved turn_count + 1 to miss the current artifact"
    );
    assert!(
        !session_dir
            .join(csa_session::result::RESULT_FILE_NAME)
            .exists(),
        "test setup requires missing root result.toml"
    );

    let mut emitted_completion: Option<(String, String, i32, bool)> = None;
    let exit_code = handle_session_wait_with_hooks(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        |_project_root, _current_session_id, _trigger| {
            Ok(WaitReconciliationOutcome {
                result_became_available: false,
                synthetic: false,
            })
        },
        |sid: &str, status: &str, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    )
    .expect("wait should detect persisted current turn artifact fallback");

    assert_eq!(exit_code, 0);
    assert_eq!(
        emitted_completion,
        Some((session_id, "success".to_string(), 0, false))
    );
}

#[test]
fn handle_session_wait_accepts_nested_manager_sidecar_current_turn_fallback() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = ScopedEnvVarRestore::set("HOME", td.path());
    let _state_guard = ScopedEnvVarRestore::set("XDG_STATE_HOME", &state_home);
    let _contract_guard = ScopedEnvVarRestore::unset(csa_session::RESULT_TOML_PATH_CONTRACT_ENV);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-nested-manager-sidecar-current-turn"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_id = session.meta_session_id.clone();
    let session_dir = get_session_dir(project, &session_id).expect("session dir");
    let current_turn_artifact = csa_session::turn_contract_result_artifact_path(1);
    let current_turn_path = session_dir.join(&current_turn_artifact);
    std::fs::create_dir_all(current_turn_path.parent().expect("turn result parent"))
        .expect("create turn result dir");
    let nested_result_toml = r#"[result]
status = "success"
summary = "nested manager sidecar completed"

[report]
what_was_done = "exercised wait fallback nested schema"

[tool]
name = "codex"
"#;
    std::fs::write(&current_turn_path, nested_result_toml).expect("write nested turn result");
    let parsed =
        crate::session_cmds_daemon::parse_output_result_artifact_for_test(nested_result_toml)
            .expect("nested manager sidecar parser should derive a wait result");
    assert_eq!(parsed.status, "success");
    assert_eq!(parsed.exit_code, 0);
    assert_eq!(parsed.summary, "nested manager sidecar completed");
    assert_eq!(parsed.tool, "codex");

    let marker_contents = format!("artifact_path = \"{current_turn_artifact}\"\n");
    std::fs::write(
        crate::pipeline::result_contract::current_result_artifact_marker_path(&session_dir),
        marker_contents,
    )
    .expect("write current result artifact marker");
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .expect("write daemon completion");
    assert!(
        !session_dir
            .join(csa_session::result::RESULT_FILE_NAME)
            .exists(),
        "test setup requires missing root result.toml"
    );

    let mut emitted_completion: Option<(String, String, i32, bool)> = None;
    let exit_code = handle_session_wait_with_hooks(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        |_project_root, _current_session_id, _trigger| {
            Ok(WaitReconciliationOutcome {
                result_became_available: false,
                synthetic: false,
            })
        },
        |sid: &str, status: &str, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    )
    .expect("wait should recover nested current turn manager sidecar fallback");

    assert_eq!(exit_code, 0);
    assert_eq!(
        emitted_completion,
        Some((session_id, "success".to_string(), 0, false))
    );
}

#[test]
fn handle_session_wait_rejects_malformed_nested_manager_sidecar_current_turn_fallback() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = ScopedEnvVarRestore::set("HOME", td.path());
    let _state_guard = ScopedEnvVarRestore::set("XDG_STATE_HOME", &state_home);
    let _contract_guard = ScopedEnvVarRestore::unset(csa_session::RESULT_TOML_PATH_CONTRACT_ENV);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-malformed-nested-manager-sidecar-current-turn"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_id = session.meta_session_id.clone();
    let session_dir = get_session_dir(project, &session_id).expect("session dir");
    let current_turn_artifact = csa_session::turn_contract_result_artifact_path(1);
    let current_turn_path = session_dir.join(&current_turn_artifact);
    std::fs::create_dir_all(current_turn_path.parent().expect("turn result parent"))
        .expect("create turn result dir");
    let malformed_nested_result_toml = r#"[result]
status = 1
summary = "non-string status must not become success"
"#;
    std::fs::write(&current_turn_path, malformed_nested_result_toml)
        .expect("write malformed nested turn result");
    let marker_contents = format!("artifact_path = \"{current_turn_artifact}\"\n");
    std::fs::write(
        crate::pipeline::result_contract::current_result_artifact_marker_path(&session_dir),
        marker_contents,
    )
    .expect("write current result artifact marker");
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 0\nstatus = \"success\"\n",
    )
    .expect("write daemon completion");
    assert!(
        !session_dir
            .join(csa_session::result::RESULT_FILE_NAME)
            .exists(),
        "test setup requires missing root result.toml"
    );

    let mut emitted_completion = false;
    let err = handle_session_wait_with_hooks(
        session_id,
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        |_project_root, _current_session_id, _trigger| {
            Ok(WaitReconciliationOutcome {
                result_became_available: false,
                synthetic: false,
            })
        },
        |_sid: &str, _status: &str, _exit_code, _synthetic, _mirror_to_stdout| {
            emitted_completion = true;
        },
    )
    .expect_err("malformed nested manager sidecar must fail closed");

    let message = err.to_string();
    assert!(
        message.contains("Failed to parse manager result artifact fallback")
            || message.contains("manager [result].status must be a string"),
        "unexpected error: {message}"
    );
    assert!(
        !emitted_completion,
        "malformed nested manager sidecar must not emit terminal success"
    );
}

#[test]
fn handle_session_wait_uses_legacy_output_result_when_current_marker_artifact_missing() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = ScopedEnvVarRestore::set("HOME", td.path());
    let _state_guard = ScopedEnvVarRestore::set("XDG_STATE_HOME", &state_home);
    let _contract_guard = ScopedEnvVarRestore::unset(csa_session::RESULT_TOML_PATH_CONTRACT_ENV);
    let project = td.path();

    let session = create_session(
        project,
        Some("wait-legacy-output-result-current-marker"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_id = session.meta_session_id.clone();
    let session_dir = get_session_dir(project, &session_id).expect("session dir");
    let current_turn_artifact = csa_session::turn_contract_result_artifact_path(1);
    let marker_contents = format!("artifact_path = \"{current_turn_artifact}\"\n");
    std::fs::write(
        crate::pipeline::result_contract::current_result_artifact_marker_path(&session_dir),
        marker_contents,
    )
    .expect("write current result artifact marker");
    let legacy_output_result_path = session_dir.join(csa_session::CONTRACT_RESULT_ARTIFACT_PATH);
    std::fs::create_dir_all(
        legacy_output_result_path
            .parent()
            .expect("legacy result parent"),
    )
    .expect("create legacy result parent");
    let legacy_output_result = SessionResult {
        summary: "current invocation wrote legacy manager result".to_string(),
        ..make_result("success", 0)
    };
    std::fs::write(
        &legacy_output_result_path,
        toml::to_string_pretty(&legacy_output_result).expect("serialize legacy result"),
    )
    .expect("write legacy output result");
    assert!(
        !session_dir.join(&current_turn_artifact).exists(),
        "test setup requires missing marker-selected current turn artifact"
    );
    assert!(
        !session_dir
            .join(csa_session::result::RESULT_FILE_NAME)
            .exists(),
        "test setup requires missing root result.toml"
    );

    let mut emitted_completion: Option<(String, String, i32, bool)> = None;
    let exit_code = handle_session_wait_with_hooks(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        |_project_root, _current_session_id, _trigger| {
            Ok(WaitReconciliationOutcome {
                result_became_available: false,
                synthetic: false,
            })
        },
        |sid: &str, status: &str, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    )
    .expect("wait should recover current legacy output/result.toml fallback");

    assert_eq!(exit_code, 0);
    assert_eq!(
        emitted_completion,
        Some((session_id, "success".to_string(), 0, false))
    );
}

#[test]
fn handle_session_wait_rejects_stale_legacy_output_result_without_current_marker() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = ScopedEnvVarRestore::set("HOME", td.path());
    let _state_guard = ScopedEnvVarRestore::set("XDG_STATE_HOME", &state_home);
    let _contract_guard = ScopedEnvVarRestore::unset(csa_session::RESULT_TOML_PATH_CONTRACT_ENV);
    let project = td.path();

    let mut session = create_session(
        project,
        Some("wait-stale-legacy-output-result"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_id = session.meta_session_id.clone();
    let session_dir = get_session_dir(project, &session_id).expect("session dir");
    let legacy_output_result_path = session_dir.join(csa_session::CONTRACT_RESULT_ARTIFACT_PATH);
    std::fs::create_dir_all(
        legacy_output_result_path
            .parent()
            .expect("legacy result parent"),
    )
    .expect("create legacy result parent");
    let stale_legacy_result = SessionResult {
        summary: "stale legacy manager result from an earlier turn".to_string(),
        ..make_result("success", 0)
    };
    std::fs::write(
        &legacy_output_result_path,
        toml::to_string_pretty(&stale_legacy_result).expect("serialize stale legacy result"),
    )
    .expect("write stale legacy output result");
    session.turn_count = 1;
    save_session(&session).expect("save state after earlier turn");
    assert!(
        !crate::pipeline::result_contract::current_result_artifact_marker_path(&session_dir)
            .exists(),
        "test setup requires no current-turn marker"
    );
    assert!(
        !csa_session::turn_contract_result_path(&session_dir, 2).exists(),
        "test setup requires missing next-turn artifact"
    );
    assert!(
        !session_dir
            .join(csa_session::result::RESULT_FILE_NAME)
            .exists(),
        "test setup requires missing root result.toml"
    );

    let mut emitted_completion = false;
    let exit_code = handle_session_wait_with_hooks(
        session_id,
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming::default(),
        },
        |_project_root, _current_session_id, _trigger| {
            Ok(WaitReconciliationOutcome {
                result_became_available: false,
                synthetic: false,
            })
        },
        |_sid: &str, _status: &str, _exit_code, _synthetic, _mirror_to_stdout| {
            emitted_completion = true;
        },
    )
    .expect("wait should reject stale legacy output/result.toml fallback");

    assert_eq!(exit_code, 1);
    assert!(
        !emitted_completion,
        "stale legacy output/result.toml must not emit terminal completion"
    );
}

#[test]
fn handle_session_wait_rejects_stale_inherited_contract_env_prior_turn_sidecar() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = ScopedEnvVarRestore::set("HOME", td.path());
    let _state_guard = ScopedEnvVarRestore::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let mut session = create_session(
        project,
        Some("wait-stale-inherited-contract-env"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_id = session.meta_session_id.clone();
    let session_dir = get_session_dir(project, &session_id).expect("session dir");
    let prior_turn_artifact = csa_session::turn_contract_result_artifact_path(1);
    let prior_turn_path = session_dir.join(&prior_turn_artifact);
    std::fs::create_dir_all(prior_turn_path.parent().expect("turn result parent"))
        .expect("create prior turn result dir");
    let stale_prior_result = SessionResult {
        summary: "stale prior turn result must not be emitted".to_string(),
        ..make_result("success", 0)
    };
    std::fs::write(
        &prior_turn_path,
        toml::to_string_pretty(&stale_prior_result).expect("serialize stale prior result"),
    )
    .expect("write stale prior turn result");
    session.turn_count = 1;
    save_session(&session).expect("save state after turn one");
    let _contract_guard =
        ScopedEnvVarRestore::set(csa_session::RESULT_TOML_PATH_CONTRACT_ENV, &prior_turn_path);

    assert!(
        !crate::pipeline::result_contract::current_result_artifact_marker_path(&session_dir)
            .exists(),
        "test setup requires no current-result marker"
    );
    assert!(
        !csa_session::turn_contract_result_path(&session_dir, 2).exists(),
        "test setup requires missing next-turn artifact"
    );
    assert!(
        !session_dir
            .join(csa_session::result::RESULT_FILE_NAME)
            .exists(),
        "test setup requires missing root result.toml"
    );
    assert_eq!(
        crate::session_cmds_daemon::expected_in_flight_turn_result_artifact_path_for_test(
            &session_dir
        ),
        None,
        "stale inherited env path must not select the prior turn sidecar"
    );

    let mut emitted_completion: Option<(String, String, i32, bool)> = None;
    let exit_code = handle_session_wait_with_hooks(
        session_id,
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming {
                poll_interval: std::time::Duration::from_millis(10),
                memory_sample_interval: std::time::Duration::from_secs(15),
            },
        },
        |_project_root, _current_session_id, _trigger| {
            Ok(WaitReconciliationOutcome {
                result_became_available: false,
                synthetic: false,
            })
        },
        |sid: &str, status: &str, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    )
    .expect("wait should ignore stale inherited contract env sidecar");

    assert_eq!(exit_code, 1);
    assert_eq!(
        emitted_completion, None,
        "stale prior summary must not be emitted as terminal completion"
    );
}

#[test]
fn handle_session_wait_accepts_inherited_contract_env_matching_state_next_turn_sidecar() {
    let td = tempdir().expect("tempdir");
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = ScopedEnvVarRestore::set("HOME", td.path());
    let _state_guard = ScopedEnvVarRestore::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let mut session = create_session(
        project,
        Some("wait-current-inherited-contract-env"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_id = session.meta_session_id.clone();
    let session_dir = get_session_dir(project, &session_id).expect("session dir");
    session.turn_count = 1;
    save_session(&session).expect("save state after turn one");
    let next_turn_artifact = csa_session::turn_contract_result_artifact_path(2);
    let next_turn_path = session_dir.join(&next_turn_artifact);
    std::fs::create_dir_all(next_turn_path.parent().expect("turn result parent"))
        .expect("create next turn result dir");
    let current_result = SessionResult {
        summary: "state-derived current turn result".to_string(),
        ..make_result("success", 0)
    };
    std::fs::write(
        &next_turn_path,
        toml::to_string_pretty(&current_result).expect("serialize current result"),
    )
    .expect("write state-derived current result");
    let _contract_guard =
        ScopedEnvVarRestore::set(csa_session::RESULT_TOML_PATH_CONTRACT_ENV, &next_turn_path);

    assert!(
        !crate::pipeline::result_contract::current_result_artifact_marker_path(&session_dir)
            .exists(),
        "test setup uses state-derived current path, not marker"
    );
    assert!(
        !session_dir
            .join(csa_session::result::RESULT_FILE_NAME)
            .exists(),
        "test setup requires missing root result.toml"
    );
    assert_eq!(
        crate::session_cmds_daemon::expected_in_flight_turn_result_artifact_path_for_test(
            &session_dir
        ),
        Some(next_turn_artifact),
        "current inherited env path should be accepted when it matches state-derived next turn"
    );

    let mut emitted_completion: Option<(String, String, i32, bool)> = None;
    let exit_code = handle_session_wait_with_hooks(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        WaitBehavior {
            wait_timeout_secs: 1,
            memory_warn_mb: None,
            timing: WaitLoopTiming {
                poll_interval: std::time::Duration::from_millis(10),
                memory_sample_interval: std::time::Duration::from_secs(15),
            },
        },
        |_project_root, _current_session_id, _trigger| {
            Ok(WaitReconciliationOutcome {
                result_became_available: false,
                synthetic: false,
            })
        },
        |sid: &str, status: &str, exit_code, synthetic, _mirror_to_stdout| {
            emitted_completion = Some((sid.to_string(), status.to_string(), exit_code, synthetic));
        },
    )
    .expect("wait should accept current inherited contract env sidecar");

    assert_eq!(exit_code, 0);
    assert_eq!(
        emitted_completion,
        Some((session_id, "success".to_string(), 0, false))
    );
}
