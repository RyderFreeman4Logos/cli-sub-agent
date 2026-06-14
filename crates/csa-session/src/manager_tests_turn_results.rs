use super::*;
use crate::post_exec_gate_report::GATE_FAILURE_LOG_REL_PATH;
use tempfile::tempdir;

fn runtime_result(summary: &str, artifact_path: &str) -> crate::result::SessionResult {
    let now = chrono::Utc::now();
    crate::result::SessionResult {
        post_exec_gate: None,
        status: "success".to_string(),
        exit_code: 0,
        summary: summary.to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 1,
        artifacts: vec![crate::result::SessionArtifact::new(artifact_path)],
        ..Default::default()
    }
}

#[test]
fn test_turn_scoped_manager_sidecars_do_not_clobber_prior_resume_reports() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();
    let session_dir = get_session_dir_in(td.path(), &state.meta_session_id);
    let turn_one_artifact = manager_result::turn_contract_result_artifact_path(1);
    let turn_two_artifact = manager_result::turn_contract_result_artifact_path(2);
    let turn_one_path = session_dir.join(&turn_one_artifact);
    let turn_two_path = session_dir.join(&turn_two_artifact);

    std::fs::create_dir_all(turn_one_path.parent().unwrap()).unwrap();
    std::fs::write(
        &turn_one_path,
        r#"
[report]
what_was_done = "turn one report"

[artifacts]
repo_write_audit = "turn-one"
"#,
    )
    .unwrap();
    save_result_in(
        td.path(),
        &state.meta_session_id,
        &runtime_result("runtime turn one", &turn_one_artifact),
        crate::SaveOptions::default(),
    )
    .unwrap();
    let turn_one_contents = std::fs::read_to_string(&turn_one_path).unwrap();
    assert!(turn_one_contents.contains("turn one report"));

    std::fs::create_dir_all(turn_two_path.parent().unwrap()).unwrap();
    std::fs::write(
        &turn_two_path,
        r#"
[report]
what_was_done = "turn two report"

[artifacts]
repo_write_audit = "turn-two"
"#,
    )
    .unwrap();
    save_result_in(
        td.path(),
        &state.meta_session_id,
        &runtime_result("runtime turn two", &turn_two_artifact),
        crate::SaveOptions::default(),
    )
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(&turn_one_path).unwrap(),
        turn_one_contents,
        "saving turn two must not overwrite turn one's manager report"
    );
    assert!(
        std::fs::read_to_string(&turn_two_path)
            .unwrap()
            .contains("turn two report")
    );

    let persisted = std::fs::read_to_string(session_dir.join(crate::result::RESULT_FILE_NAME))
        .expect("runtime result should exist");
    assert!(persisted.contains("status = \"success\""));
    assert!(persisted.contains("summary = \"runtime turn two\""));
    assert!(persisted.contains(&turn_two_artifact));
    assert!(
        !persisted.contains(&turn_one_artifact),
        "runtime envelope should reference only the current manager sidecar"
    );

    let loaded = load_result_in(td.path(), &state.meta_session_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        loaded
            .manager_fields
            .report
            .as_ref()
            .and_then(|value| value.get("what_was_done")),
        Some(&toml::Value::String("turn two report".to_string()))
    );
}

#[test]
fn test_turn_scoped_sidecar_preservation_ignores_stale_legacy_sidecar() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();
    let session_dir = get_session_dir_in(td.path(), &state.meta_session_id);
    let legacy_path = manager_result::contract_result_path(&session_dir);
    let turn_artifact = manager_result::turn_contract_result_artifact_path(2);
    let turn_path = session_dir.join(&turn_artifact);

    std::fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
    std::fs::write(
        &legacy_path,
        r#"
[report]
what_was_done = "stale legacy report"

[artifacts]
repo_write_audit = "legacy-stale"
"#,
    )
    .unwrap();
    std::fs::create_dir_all(turn_path.parent().unwrap()).unwrap();
    std::fs::write(
        &turn_path,
        r#"
[report]
what_was_done = "current turn report"

[artifacts]
repo_write_audit = "turn-current"
"#,
    )
    .unwrap();

    save_result_in(
        td.path(),
        &state.meta_session_id,
        &runtime_result("runtime turn two", &turn_artifact),
        crate::SaveOptions::default(),
    )
    .unwrap();

    let turn_contents = std::fs::read_to_string(&turn_path).unwrap();
    assert!(
        turn_contents.contains("current turn report"),
        "selected turn sidecar must remain the preserved manager report"
    );
    assert!(
        turn_contents.contains("turn-current"),
        "selected turn sidecar artifacts must remain current turn fields"
    );
    assert!(
        !turn_contents.contains("stale legacy report"),
        "stale legacy sidecar must not be republished into the selected turn path"
    );
    assert!(
        !turn_contents.contains("legacy-stale"),
        "stale legacy artifact fields must not replace current turn fields"
    );
    assert!(
        std::fs::read_to_string(&legacy_path)
            .unwrap()
            .contains("stale legacy report"),
        "legacy sidecar may coexist but must not drive turn preservation"
    );

    let loaded = load_result_in(td.path(), &state.meta_session_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        loaded
            .manager_fields
            .report
            .as_ref()
            .and_then(|value| value.get("what_was_done")),
        Some(&toml::Value::String("current turn report".to_string()))
    );
    assert!(
        loaded
            .artifacts
            .iter()
            .any(|artifact| artifact.path == turn_artifact)
    );
    assert!(
        loaded
            .artifacts
            .iter()
            .all(|artifact| artifact.path != manager_result::CONTRACT_RESULT_ARTIFACT_PATH),
        "runtime envelope should advertise the selected turn sidecar, not legacy output/result.toml"
    );
}

#[test]
fn test_new_manager_sidecar_without_explicit_turn_artifact_preserves_prior_turn_report() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();
    let session_dir = get_session_dir_in(td.path(), &state.meta_session_id);
    let prior_turn_artifact = manager_result::turn_contract_result_artifact_path(1);
    let prior_turn_path = session_dir.join(&prior_turn_artifact);

    std::fs::create_dir_all(prior_turn_path.parent().unwrap()).unwrap();
    std::fs::write(
        &prior_turn_path,
        r#"
[report]
what_was_done = "prior turn report"
"#,
    )
    .unwrap();
    let prior_turn_contents = std::fs::read_to_string(&prior_turn_path).unwrap();
    let now = chrono::Utc::now();
    let result = crate::result::SessionResult {
        post_exec_gate: None,
        status: "success".to_string(),
        exit_code: 0,
        summary: "runtime summary".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 1,
        artifacts: vec![],
        manager_fields: crate::result::SessionManagerFields {
            report: Some(toml::toml! { what_was_done = "legacy fallback report" }.into()),
            ..Default::default()
        },
        ..Default::default()
    };

    save_result_in(
        td.path(),
        &state.meta_session_id,
        &result,
        crate::SaveOptions::default(),
    )
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(&prior_turn_path).unwrap(),
        prior_turn_contents,
        "a save without an explicit turn artifact must not overwrite a prior turn report"
    );
    assert!(
        std::fs::read_to_string(manager_result::contract_result_path(&session_dir))
            .unwrap()
            .contains("legacy fallback report"),
        "legacy sidecar should receive manager fields when no turn artifact is explicit"
    );

    let loaded = load_result_in(td.path(), &state.meta_session_id)
        .unwrap()
        .unwrap();
    assert!(
        loaded
            .artifacts
            .iter()
            .any(|artifact| artifact.path == manager_result::CONTRACT_RESULT_ARTIFACT_PATH)
    );
}

#[test]
fn test_expected_turn_artifact_helper_does_not_return_prior_turn_result() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();
    let session_dir = get_session_dir_in(td.path(), &state.meta_session_id);
    let turn_one_path = manager_result::turn_contract_result_path(&session_dir, 1);

    std::fs::create_dir_all(turn_one_path.parent().unwrap()).unwrap();
    std::fs::write(
        &turn_one_path,
        "status = \"success\"\nsummary = \"turn one\"\n",
    )
    .unwrap();

    assert_eq!(
        manager_result::existing_next_turn_contract_result_artifact_path(&session_dir, 0),
        Some(manager_result::turn_contract_result_artifact_path(1))
    );
    assert_eq!(
        manager_result::existing_next_turn_contract_result_artifact_path(&session_dir, 1),
        None,
        "turn 1 must not satisfy the expected turn 2 artifact lookup"
    );
}

#[test]
fn test_save_result_without_explicit_sidecar_does_not_attach_prior_turn_artifact() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();
    let session_dir = get_session_dir_in(td.path(), &state.meta_session_id);
    let prior_turn_artifact = manager_result::turn_contract_result_artifact_path(1);
    let prior_turn_path = session_dir.join(&prior_turn_artifact);

    std::fs::create_dir_all(prior_turn_path.parent().unwrap()).unwrap();
    std::fs::write(
        &prior_turn_path,
        r#"
[report]
what_was_done = "prior turn report"
"#,
    )
    .unwrap();

    let now = chrono::Utc::now();
    let result = crate::result::SessionResult {
        post_exec_gate: None,
        status: "failure".to_string(),
        exit_code: 1,
        summary: "turn two failed before writing sidecar".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 1,
        artifacts: vec![],
        ..Default::default()
    };

    save_result_in(
        td.path(),
        &state.meta_session_id,
        &result,
        crate::SaveOptions::default(),
    )
    .unwrap();

    let persisted = std::fs::read_to_string(session_dir.join(crate::result::RESULT_FILE_NAME))
        .expect("runtime result should exist");
    assert!(
        !persisted.contains(&prior_turn_artifact),
        "a new envelope without an explicit sidecar must not advertise a prior turn artifact"
    );

    let loaded = load_result_in(td.path(), &state.meta_session_id)
        .unwrap()
        .unwrap();
    assert!(
        loaded.manager_fields.as_sidecar().is_none(),
        "prior turn manager fields must not be rehydrated into the new root result"
    );
}

#[test]
fn test_display_only_turn_result_artifact_is_visible_but_not_selected_as_manager_sidecar() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();
    let session_dir = get_session_dir_in(td.path(), &state.meta_session_id);
    let owned_artifact = manager_result::turn_contract_result_artifact_path(1);
    let discovered_artifact = manager_result::turn_contract_result_artifact_path(2);
    let owned_path = session_dir.join(&owned_artifact);
    let discovered_path = session_dir.join(&discovered_artifact);

    std::fs::create_dir_all(owned_path.parent().unwrap()).unwrap();
    std::fs::write(
        &owned_path,
        r#"
[report]
what_was_done = "owned turn one report"
"#,
    )
    .unwrap();
    std::fs::create_dir_all(discovered_path.parent().unwrap()).unwrap();
    std::fs::write(
        &discovered_path,
        r#"
[report]
what_was_done = "in-flight turn two report"
"#,
    )
    .unwrap();
    let discovered_before = std::fs::read_to_string(&discovered_path).unwrap();

    let mut result = runtime_result("runtime turn one", &owned_artifact);
    result
        .artifacts
        .push(crate::result::SessionArtifact::display_only(
            discovered_artifact.clone(),
        ));
    save_result_in(
        td.path(),
        &state.meta_session_id,
        &result,
        crate::SaveOptions::default(),
    )
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(&discovered_path).unwrap(),
        discovered_before,
        "display-only turn result artifact must not be rewritten as a manager sidecar"
    );

    let loaded = load_result_in(td.path(), &state.meta_session_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        loaded
            .manager_fields
            .report
            .as_ref()
            .and_then(|value| value.get("what_was_done")),
        Some(&toml::Value::String("owned turn one report".to_string())),
        "manager overlay must come from the owned turn sidecar, not the display-only artifact"
    );
    assert!(
        loaded
            .artifacts
            .iter()
            .any(|artifact| artifact.path == discovered_artifact && artifact.display_only),
        "display-only turn artifact must remain visible in the envelope"
    );
}

#[test]
fn test_observed_session_artifact_marks_manager_results_display_only() {
    let legacy = observed_session_artifact(CONTRACT_RESULT_ARTIFACT_PATH);
    assert_eq!(legacy.path, CONTRACT_RESULT_ARTIFACT_PATH);
    assert!(
        legacy.display_only,
        "observed legacy manager result artifacts are diagnostics-only"
    );

    let turn_artifact = turn_contract_result_artifact_path(7);
    let observed_turn = observed_session_artifact(turn_artifact.clone());
    assert_eq!(observed_turn.path, turn_artifact);
    assert!(
        observed_turn.display_only,
        "observed turn-scoped manager result artifacts are diagnostics-only"
    );

    let observed_log = observed_session_artifact("output/acp-events.jsonl");
    assert_eq!(observed_log.path, "output/acp-events.jsonl");
    assert!(
        !observed_log.display_only,
        "ordinary observed artifacts remain owned artifacts"
    );

    let observed_gate_log = observed_session_artifact(GATE_FAILURE_LOG_REL_PATH);
    assert_eq!(observed_gate_log.path, GATE_FAILURE_LOG_REL_PATH);
    assert!(
        observed_gate_log.display_only,
        "observed gate-failure logs must not prove current result ownership"
    );
}
