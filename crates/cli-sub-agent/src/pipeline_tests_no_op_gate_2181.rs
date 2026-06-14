//! Regression coverage for GitHub issue #2181: commit-skill no-op failures
//! must preserve actionable, bounded, redacted request context.

use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_executor::Executor;
use csa_session::{SessionResult, create_session, load_result};

fn build_commit_no_op_ctx<'a>(
    executor: &'a Executor,
    session_dir: std::path::PathBuf,
    project_root: &'a std::path::Path,
    execution_start_time: chrono::DateTime<chrono::Utc>,
    hooks_config: &'a csa_hooks::HooksConfig,
    prompt: &'a str,
) -> PostExecContext<'a> {
    PostExecContext {
        executor,
        prompt,
        effective_prompt: prompt,
        task_type: Some("run"),
        readonly_project_root: false,
        project_root,
        config: None,
        global_config: None,
        session_dir,
        sessions_root: "test-root".to_string(),
        execution_start_time,
        hooks_config,
        memory_project_key: None,
        provider_session_id: None,
        events_count: 4,
        transcript_artifacts: vec![],
        changed_paths: vec![],
        pre_exec_snapshot: None,
        has_tool_calls: false,
        turn_count: 0,
        output_tokens: None,
        sa_mode: true,
    }
}

fn build_successful_empty_result(summary: &str) -> csa_process::ExecutionResult {
    csa_process::ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: summary.to_string(),
        exit_code: 0,
        peak_memory_mb: None,
        ..Default::default()
    }
}

#[tokio::test]
async fn no_op_gate_for_commit_skill_includes_actionable_redacted_request_context() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    let mut session = create_session(
        project_root,
        Some("commit vllm dflash tool parser"),
        None,
        Some("openai-compat"),
    )
    .expect("create");
    let session_dir =
        csa_session::get_session_dir(project_root, &session.meta_session_id).expect("dir");

    let request = format!(
        "Commit the current changes with scope: api_key=sk-sup...2345 \
         enable DFlash vLLM auto tool choice for Hermes and document swap/parser \
         findings. Do not push or create PR; create only the local commit. {}",
        "include detailed staged-file analysis ".repeat(20),
    );
    let prompt = format!(
        "<skill-mode>executor</skill-mode>\n\n\
         <skill-source path=\"{}/patterns/commit/skills/commit\">\n\
         Resolve relative skill references from this directory.\n\
         </skill-source>\n\n\
         # Commit Skill\nCommit with audit metadata.\n\n---\n\n{}",
        project_root.display(),
        request,
    );

    let executor = Executor::OpenaiCompat {
        model_override: None,
        thinking_budget: None,
    };
    let hooks_config = csa_hooks::HooksConfig::default();
    let start = chrono::Utc::now() - chrono::Duration::seconds(10);
    let ctx = build_commit_no_op_ctx(
        &executor,
        session_dir,
        project_root,
        start,
        &hooks_config,
        &prompt,
    );
    let mut result = build_successful_empty_result("and the required AI Reviewer Metadata block.");

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process_execution_result");

    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load")
        .expect("result exists");
    assert_eq!(persisted.exit_code, 1);
    assert_eq!(persisted.status, SessionResult::status_from_exit_code(1));
    assert_eq!(persisted.summary, result.summary);
    assert!(
        persisted.summary.starts_with("no-op exit detected"),
        "summary should start with no-op diagnostic, got: {}",
        persisted.summary
    );
    assert!(persisted.summary.contains("turn_count=1"));
    assert!(persisted.summary.contains("no tool calls"));
    assert!(persisted.summary.contains("tool=openai-compat"));
    assert!(
        persisted
            .summary
            .contains("commit skill requested task did not run"),
        "summary must say the commit skill did not run: {}",
        persisted.summary
    );
    assert!(persisted.summary.contains("commit vllm dflash tool parser"));
    assert!(
        persisted
            .summary
            .contains("enable DFlash vLLM auto tool choice"),
        "summary must preserve the useful head of the original request, not only the tail: {}",
        persisted.summary
    );
    assert!(
        persisted.summary.contains("rerun csa run --skill commit"),
        "summary must include a direct recovery hint: {}",
        persisted.summary
    );
    assert!(
        persisted.summary.contains("no local commit"),
        "commit-skill no-op must make fail-closed side effect clear: {}",
        persisted.summary
    );
    assert!(
        persisted.summary.contains("[REDACTED]"),
        "request excerpt should be redacted: {}",
        persisted.summary
    );
    assert!(!persisted.summary.contains("sk-sup...2345"));
    assert!(
        persisted.summary.chars().count() <= 500,
        "summary must remain bounded for wait/result surfaces ({} chars): {}",
        persisted.summary.chars().count(),
        persisted.summary
    );

    let json = serde_json::to_value(&result).expect("ExecutionResult should serialize");
    assert_eq!(json["summary"].as_str(), Some(persisted.summary.as_str()));
    assert_eq!(json["csa_gate_failure"].as_str(), Some("no-op-exit"));
    assert!(
        json["summary"]
            .as_str()
            .expect("json summary")
            .contains("enable DFlash vLLM auto tool choice"),
        "--format json surface must carry the actionable no-op summary"
    );
    assert!(
        !json["summary"]
            .as_str()
            .expect("json summary")
            .contains("sk-sup...2345"),
        "--format json summary must be redacted"
    );
}
