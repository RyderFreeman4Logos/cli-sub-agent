use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_session::{SessionResult, create_session, get_session_dir, load_result, save_result};
use std::fs;

#[test]
fn current_marker_upgrades_observed_turn_sidecar_to_owned_manager_artifact() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project_root = tmp.path();
    let session =
        create_session(project_root, Some("test"), None, Some("codex")).expect("create session");
    let session_dir = get_session_dir(project_root, &session.meta_session_id).expect("session dir");
    let current_artifact = csa_session::turn_contract_result_artifact_path(1);
    let historical_artifact = csa_session::turn_contract_result_artifact_path(2);
    let post_stream_artifact = csa_session::turn_contract_result_artifact_path(3);
    let current_path = session_dir.join(&current_artifact);
    let historical_path = session_dir.join(&historical_artifact);

    fs::create_dir_all(current_path.parent().expect("current parent"))
        .expect("create current parent");
    fs::write(
        &current_path,
        r#"
[report]
what_was_done = "current marker-owned report"
"#,
    )
    .expect("write current sidecar");
    fs::create_dir_all(historical_path.parent().expect("historical parent"))
        .expect("create historical parent");
    fs::write(
        &historical_path,
        r#"
[report]
what_was_done = "historical diagnostic report"
"#,
    )
    .expect("write historical sidecar");
    fs::write(
        crate::pipeline::result_contract::current_result_artifact_marker_path(&session_dir),
        format!("artifact_path = \"{current_artifact}\"\n"),
    )
    .expect("write current marker");

    let now = chrono::Utc::now();
    let mut result = SessionResult {
        post_exec_gate: None,
        status: "success".to_string(),
        exit_code: 0,
        summary: "runtime envelope".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 1,
        artifacts: Vec::new(),
        ..Default::default()
    };

    crate::session_observability::enrich_result_from_session_dir(
        project_root,
        &session.meta_session_id,
        &session_dir,
        &mut result,
    )
    .expect("refresh artifacts");
    assert!(
        result
            .artifacts
            .iter()
            .any(|artifact| artifact.path == current_artifact && artifact.display_only),
        "recursive artifact refresh should first observe the current sidecar as display-only"
    );
    assert!(
        result
            .artifacts
            .iter()
            .any(|artifact| artifact.path == historical_artifact && artifact.display_only),
        "historical manager sidecars should remain display-only diagnostics"
    );
    result.artifacts.push(csa_session::SessionArtifact::new(
        post_stream_artifact.clone(),
    ));

    ensure_turn_scoped_manager_artifact(&session_dir, 3, &mut result);
    assert!(
        result
            .artifacts
            .iter()
            .any(|artifact| artifact.path == current_artifact && !artifact.display_only),
        "current marker path should be upgraded to an owned manager artifact"
    );
    assert!(
        result
            .artifacts
            .iter()
            .any(|artifact| artifact.path == historical_artifact && artifact.display_only),
        "unproven historical sidecar must stay display-only"
    );
    assert!(
        result
            .artifacts
            .iter()
            .all(|artifact| artifact.path != post_stream_artifact),
        "marker-owned sidecar must be the only owned manager result artifact"
    );

    save_result(project_root, &session.meta_session_id, &result).expect("save result");
    let loaded = load_result(project_root, &session.meta_session_id)
        .expect("load result")
        .expect("result should exist");
    assert_eq!(
        loaded
            .manager_fields
            .report
            .as_ref()
            .and_then(|value| value.get("what_was_done")),
        Some(&toml::Value::String(
            "current marker-owned report".to_string()
        )),
        "load_result should overlay the marker-owned current sidecar"
    );
    assert!(
        loaded
            .artifacts
            .iter()
            .any(|artifact| artifact.path == historical_artifact && artifact.display_only),
        "historical sidecar remains visible but diagnostics-only after save/load"
    );
}
