#[test]
fn test_save_result_preserves_existing_contract_result_artifact_when_output_result_exists() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();
    let session_dir = get_session_dir_in(td.path(), &state.meta_session_id);
    let sidecar_path = manager_result::contract_result_path(&session_dir);
    std::fs::write(&sidecar_path, "status = \"success\"\nsummary = \"manager-facing report\"\n")
        .unwrap();

    let now = chrono::Utc::now();
    let runtime_result = crate::result::SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "runtime summary".to_string(),
        tool: "codex".to_string(),
        started_at: now,
        completed_at: now,
        events_count: 1,
        artifacts: vec![crate::result::SessionArtifact::new("output/acp-events.jsonl")],
        peak_memory_mb: None,
        manager_fields: Default::default(),
    };
    save_result_in(
        td.path(),
        &state.meta_session_id,
        &runtime_result,
        crate::SaveOptions::default(),
    )
    .unwrap();

    let loaded = load_result_in(td.path(), &state.meta_session_id)
        .unwrap()
        .unwrap();
    assert!(sidecar_path.exists());
    let sidecar_contents = std::fs::read_to_string(&sidecar_path).unwrap();
    assert!(sidecar_contents.contains("manager-facing report"));
    assert!(
        loaded
            .artifacts
            .iter()
            .any(|artifact| artifact.path == manager_result::CONTRACT_RESULT_ARTIFACT_PATH)
    );
}

#[cfg(unix)]
#[test]
fn sidecar_write_failure_leaves_envelope_unchanged() {
    use std::os::unix::fs::PermissionsExt;

    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();
    let session_dir = get_session_dir_in(td.path(), &state.meta_session_id);
    let result_path = session_dir.join(crate::result::RESULT_FILE_NAME);
    let sidecar_path = manager_result::contract_result_path(&session_dir);
    let output_dir = session_dir.join("output");

    let now = chrono::Utc::now();
    let initial_result = crate::result::SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "initial summary".to_string(),
        tool: "codex".to_string(),
        started_at: now,
        completed_at: now,
        events_count: 1,
        artifacts: vec![],
        peak_memory_mb: None,
        manager_fields: crate::result::SessionManagerFields {
            artifacts: Some(
                toml::toml! {
                    [repo_write_audit]
                    added = ["before.txt"]
                }
                .into(),
            ),
            ..Default::default()
        },
    };
    save_result_in(
        td.path(),
        &state.meta_session_id,
        &initial_result,
        crate::SaveOptions::default(),
    )
    .unwrap();

    let envelope_before = std::fs::read_to_string(&result_path).unwrap();
    let sidecar_before = std::fs::read_to_string(&sidecar_path).unwrap();

    let original_permissions = std::fs::metadata(&output_dir).unwrap().permissions();
    std::fs::set_permissions(&output_dir, std::fs::Permissions::from_mode(0o555)).unwrap();

    let updated_result = crate::result::SessionResult {
        summary: "updated summary".to_string(),
        manager_fields: crate::result::SessionManagerFields {
            artifacts: Some(
                toml::toml! {
                    [repo_write_audit]
                    added = ["after.txt"]
                }
                .into(),
            ),
            ..Default::default()
        },
        ..initial_result
    };
    let err = save_result_in(
        td.path(),
        &state.meta_session_id,
        &updated_result,
        crate::SaveOptions::default(),
    )
    .expect_err("sidecar write should fail before envelope publication");

    std::fs::set_permissions(&output_dir, original_permissions).unwrap();

    assert!(err.to_string().contains("Failed to write result sidecar"));
    assert_eq!(std::fs::read_to_string(&result_path).unwrap(), envelope_before);
    assert_eq!(std::fs::read_to_string(&sidecar_path).unwrap(), sidecar_before);
}
