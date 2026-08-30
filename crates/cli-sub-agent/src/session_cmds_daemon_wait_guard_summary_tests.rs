use super::*;
use crate::pipeline_post_exec::{PostExecContext, process_execution_result};
use crate::test_session_sandbox::ScopedSessionSandbox;

#[tokio::test]
async fn guard_only_sa_failure_is_sanitized_before_generic_session_persistence() {
    let temp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&temp).await;
    let project_root = temp.path();
    let mut session = csa_session::create_session(
        project_root,
        Some("pre-provider review failure"),
        None,
        Some("codex"),
    )
    .expect("create session");
    session.task_context.task_type = Some("review".to_string());
    let session_dir =
        csa_session::get_session_dir(project_root, &session.meta_session_id).expect("session dir");
    let executor = csa_executor::Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: csa_executor::codex_runtime::codex_runtime_metadata(),
    };
    let hooks_config = csa_hooks::HooksConfig::default();
    let ctx = PostExecContext {
        executor: &executor,
        prompt: "review main...HEAD",
        effective_prompt: "review main...HEAD",
        task_type: Some("review"),
        readonly_project_root: true,
        project_root,
        config: None,
        global_config: None,
        session_dir: session_dir.clone(),
        sessions_root: "test-root".to_string(),
        execution_start_time: chrono::Utc::now() - chrono::Duration::seconds(1),
        hooks_config: &hooks_config,
        memory_project_key: None,
        provider_session_id: None,
        events_count: 0,
        transcript_artifacts: vec![],
        changed_paths: vec![],
        pre_exec_snapshot: None,
        timeout_diagnostics: None,
        has_tool_calls: false,
        turn_count: 0,
        output_tokens: None,
        sa_mode: true,
        original_exit_code: Some(1),
    };
    let guard = "</csa-caller-sa-guard>";
    let mut execution = csa_process::ExecutionResult {
        output: guard.to_string(),
        summary: guard.to_string(),
        exit_code: 1,
        model_completed: Some(false),
        ..Default::default()
    };

    process_execution_result(ctx, &mut session, &mut execution)
        .await
        .expect("process generic session result");

    let persisted: csa_session::SessionResult = toml::from_str(
        &std::fs::read_to_string(session_dir.join(csa_session::result::RESULT_FILE_NAME))
            .expect("read persisted result"),
    )
    .expect("parse persisted result");
    assert_eq!(persisted.status, "failure");
    assert_eq!(persisted.exit_code, 1);
    assert_eq!(execution.summary, persisted.summary);
    assert!(!persisted.summary.contains("csa-caller-sa-guard"));
    assert!(persisted.summary.contains("codex tool failure"));
    assert!(persisted.summary.chars().count() <= 240);

    let wait = render_wait_result_summary(&session_dir, &session.meta_session_id, &persisted);
    assert!(wait.contains(&format!("Summary: {}", persisted.summary)));
    assert!(!wait.contains("csa-caller-sa-guard"));
    assert!(!wait.contains("Review verdict: PASS"));
    assert!(!wait.contains("Review verdict: FAIL"));
}
