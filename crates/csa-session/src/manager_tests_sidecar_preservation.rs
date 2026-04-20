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
