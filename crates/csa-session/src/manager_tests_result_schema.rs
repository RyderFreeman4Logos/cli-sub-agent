fn result_schema_runtime_result(
    summary: &str,
    artifacts: Vec<crate::result::SessionArtifact>,
) -> crate::result::SessionResult {
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
        events_count: 0,
        artifacts,
        ..Default::default()
    }
}

#[test]
fn test_display_only_artifact_runtime_schema_does_not_create_user_result_snapshot() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();
    let session_dir = get_session_dir_in(td.path(), &state.meta_session_id);
    let result_path = session_dir.join(crate::result::RESULT_FILE_NAME);
    let display_artifact = manager_result::turn_contract_result_artifact_path(1);

    let first_result = result_schema_runtime_result(
        "display-only artifact",
        vec![crate::result::SessionArtifact::display_only(
            display_artifact.clone(),
        )],
    );
    save_result_in(
        td.path(),
        &state.meta_session_id,
        &first_result,
        crate::SaveOptions::default(),
    )
    .unwrap();

    let after_first_raw = std::fs::read_to_string(&result_path).unwrap();
    assert!(
        after_first_raw.contains("display_only = true"),
        "first save must serialize display_only as a runtime artifact field: {after_first_raw}"
    );

    let second_result = result_schema_runtime_result(
        "second clean save",
        vec![crate::result::SessionArtifact::display_only(
            display_artifact.clone(),
        )],
    );
    save_result_in(
        td.path(),
        &state.meta_session_id,
        &second_result,
        crate::SaveOptions::default(),
    )
    .unwrap();

    assert!(
        !session_dir
            .join(manager_result::LEGACY_USER_RESULT_ARTIFACT_PATH)
            .exists(),
        "a runtime artifact with boolean display_only must not be snapshotted as a custom user result"
    );
    let loaded = load_result_in(td.path(), &state.meta_session_id)
        .unwrap()
        .unwrap();
    assert!(
        loaded
            .artifacts
            .iter()
            .any(|artifact| artifact.path == display_artifact && artifact.display_only),
        "runtime result must still round-trip the display-only artifact"
    );
}

#[test]
fn test_display_only_artifact_runtime_schema_rejects_non_bool_value() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();
    let session_dir = get_session_dir_in(td.path(), &state.meta_session_id);
    let result_path = session_dir.join(crate::result::RESULT_FILE_NAME);
    let display_artifact = manager_result::turn_contract_result_artifact_path(1);

    let first_result = result_schema_runtime_result(
        "display-only artifact",
        vec![crate::result::SessionArtifact::display_only(display_artifact)],
    );
    save_result_in(
        td.path(),
        &state.meta_session_id,
        &first_result,
        crate::SaveOptions::default(),
    )
    .unwrap();

    let invalid_runtime_schema = std::fs::read_to_string(&result_path)
        .unwrap()
        .replace("display_only = true", "display_only = \"true\"");
    std::fs::write(&result_path, invalid_runtime_schema).unwrap();

    let second_result = result_schema_runtime_result("second clean save", vec![]);
    save_result_in(
        td.path(),
        &state.meta_session_id,
        &second_result,
        crate::SaveOptions::default(),
    )
    .unwrap();

    let snapshot_path = session_dir.join(manager_result::LEGACY_USER_RESULT_ARTIFACT_PATH);
    assert!(
        snapshot_path.exists(),
        "a non-bool display_only artifact field must still be treated as custom schema"
    );
    let snapshot = std::fs::read_to_string(snapshot_path).unwrap();
    assert!(
        snapshot.contains("display_only = \"true\""),
        "custom-schema snapshot must preserve the invalid display_only value: {snapshot}"
    );
}
