use super::*;

fn runtime_result(summary: &str, artifact_path: &str) -> SessionResult {
    let now = chrono::Utc::now();
    SessionResult {
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
        artifacts: vec![SessionArtifact::new(artifact_path)],
        ..Default::default()
    }
}

#[test]
fn refresh_and_repair_result_does_not_overwrite_in_flight_turn_sidecar() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let _state_guard =
        crate::test_env_lock::ScopedTestEnvVar::set("XDG_STATE_HOME", temp.path().join("state"));
    let project_root = temp.path().join("project");
    fs::create_dir_all(&project_root).expect("create project");
    let session = csa_session::create_session(
        &project_root,
        Some("refresh sidecar ownership regression"),
        None,
        Some("codex"),
    )
    .expect("create session");
    let session_dir =
        csa_session::get_session_dir(&project_root, &session.meta_session_id).expect("session dir");
    let turn_one_artifact = csa_session::turn_contract_result_artifact_path(1);
    let turn_two_artifact = csa_session::turn_contract_result_artifact_path(2);
    let turn_one_path = session_dir.join(&turn_one_artifact);
    let turn_two_path = session_dir.join(&turn_two_artifact);

    fs::create_dir_all(turn_one_path.parent().expect("turn one parent"))
        .expect("create turn one parent");
    fs::write(
        &turn_one_path,
        r#"
[report]
what_was_done = "completed turn one report"
"#,
    )
    .expect("write turn one sidecar");
    csa_session::save_result(
        &project_root,
        &session.meta_session_id,
        &runtime_result("completed turn one", &turn_one_artifact),
    )
    .expect("save completed turn one envelope");

    fs::create_dir_all(turn_two_path.parent().expect("turn two parent"))
        .expect("create turn two parent");
    fs::write(
        &turn_two_path,
        r#"
[report]
what_was_done = "in-flight turn two report"
"#,
    )
    .expect("write in-flight turn two sidecar");
    let turn_two_before = fs::read_to_string(&turn_two_path).expect("read turn two before");

    let refreshed = refresh_and_repair_result(&project_root, &session.meta_session_id)
        .expect("refresh result")
        .expect("result should exist");

    assert_eq!(
        fs::read_to_string(&turn_two_path).expect("read turn two after"),
        turn_two_before,
        "observation refresh must not overwrite an in-flight turn result sidecar"
    );
    assert!(
        fs::read_to_string(&turn_one_path)
            .expect("read turn one after")
            .contains("completed turn one report"),
        "owned turn one sidecar should remain the manager report source"
    );
    assert!(
        refreshed
            .artifacts
            .iter()
            .any(|artifact| artifact.path == turn_one_artifact && !artifact.display_only),
        "completed turn one sidecar remains the owned manager artifact"
    );
    assert!(
        refreshed
            .artifacts
            .iter()
            .any(|artifact| artifact.path == turn_two_artifact && artifact.display_only),
        "in-flight turn two sidecar remains visible but display-only"
    );

    let loaded = csa_session::load_result(&project_root, &session.meta_session_id)
        .expect("load result")
        .expect("result should exist");
    assert_eq!(
        loaded
            .manager_fields
            .report
            .as_ref()
            .and_then(|value| value.get("what_was_done")),
        Some(&toml::Value::String(
            "completed turn one report".to_string()
        )),
        "subsequent loads must ignore display-only in-flight sidecars for manager overlay"
    );
}
