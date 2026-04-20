use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;
use std::fs;

#[tokio::test]
async fn execute_with_session_and_meta_does_not_persist_runtime_binary_when_lock_is_held() {
    let temp = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&temp).await;
    let project_root = temp.path();

    let session =
        csa_session::create_session(project_root, Some("resume-target"), None, Some("codex"))
            .unwrap();
    let session_dir = csa_session::get_session_dir(project_root, &session.meta_session_id).unwrap();
    let metadata_path = session_dir.join(csa_session::metadata::METADATA_FILE_NAME);
    let metadata = csa_session::metadata::SessionMetadata {
        tool: "codex".to_string(),
        tool_locked: true,
        runtime_binary: Some("codex".to_string()),
    };
    fs::write(&metadata_path, toml::to_string_pretty(&metadata).unwrap()).unwrap();

    let _lock = csa_lock::acquire_lock(&session_dir, "codex", "active resume winner").unwrap();
    let executor = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: csa_executor::CodexRuntimeMetadata::from_transport(
            csa_executor::CodexTransport::Acp,
        ),
    };

    let execution = execute_with_session_and_meta(
        &executor,
        &ToolName::Codex,
        "resume prompt",
        csa_core::types::OutputFormat::Json,
        Some(session.meta_session_id.clone()),
        None,
        None,
        project_root,
        None,
        None,
        None,
        None,
        None,
        csa_process::StreamMode::BufferOnly,
        DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,
        None,
        None,
        None,
        false,
        false,
        &[],
        &[],
    )
    .await;
    let err = match execution {
        Ok(_) => panic!("held session lock must reject the losing resume attempt"),
        Err(err) => err,
    };

    assert!(
        err.to_string().contains("Failed to acquire lock"),
        "unexpected error: {err:#}"
    );

    let persisted = toml::from_str::<csa_session::metadata::SessionMetadata>(
        &fs::read_to_string(&metadata_path).unwrap(),
    )
    .unwrap();
    assert_eq!(
        persisted.runtime_binary.as_deref(),
        Some("codex"),
        "lock loser must not overwrite the winner's runtime_binary"
    );
}
