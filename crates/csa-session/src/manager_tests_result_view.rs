#[test]
fn test_load_result_view_surfaces_manager_and_legacy_sidecars() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();
    let session_dir = get_session_dir_in(td.path(), &state.meta_session_id);

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
    };
    save_result_in(td.path(), &state.meta_session_id, &runtime_result).unwrap();

    std::fs::write(
        session_dir.join(manager_result::CONTRACT_RESULT_ARTIFACT_PATH),
        "[report]\nsummary = \"manager-visible\"\n",
    )
    .unwrap();
    std::fs::write(
        session_dir.join(manager_result::LEGACY_USER_RESULT_ARTIFACT_PATH),
        "[artifacts]\ncount = 2\n",
    )
    .unwrap();

    let loaded = load_result_view_in(td.path(), &state.meta_session_id)
        .unwrap()
        .expect("result view should exist");
    assert_eq!(loaded.envelope.summary, "runtime summary");
    assert_eq!(
        loaded.manager_sidecar.as_ref().and_then(|value| value.get("report")),
        Some(&toml::Value::Table(
            [("summary".to_string(), toml::Value::String("manager-visible".to_string()))]
                .into_iter()
                .collect()
        ))
    );
    assert_eq!(
        loaded.legacy_sidecar.as_ref().and_then(|value| value.get("artifacts")),
        Some(&toml::Value::Table(
            [("count".to_string(), toml::Value::Integer(2))]
                .into_iter()
                .collect()
        ))
    );
}

#[test]
fn test_redact_result_sidecar_value_masks_secret_fields() {
    let redacted = manager_result::redact_result_sidecar_value(
        &toml::toml! {
            [auth]
            api_key = "hunter2"
            token = "secret-token"
        }
        .into(),
    )
    .expect("redacted sidecar");

    let rendered = toml::to_string_pretty(&redacted).expect("render redacted sidecar");
    assert!(!rendered.contains("hunter2"));
    assert!(!rendered.contains("secret-token"));
    assert!(rendered.contains("[REDACTED]"));
}
