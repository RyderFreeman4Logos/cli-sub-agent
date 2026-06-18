//! Tests for the SA-mode no-op exit gate in `process_execution_result`.

use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_executor::{ClaudeCodeRuntimeMetadata, Executor};
use csa_session::{SessionPhase, create_session, load_result, load_session};

/// Build a minimal `PostExecContext` suitable for no-op gate testing.
fn build_test_ctx<'a>(
    executor: &'a Executor,
    session_dir: std::path::PathBuf,
    project_root: &'a std::path::Path,
    execution_start_time: chrono::DateTime<chrono::Utc>,
    hooks_config: &'a csa_hooks::HooksConfig,
    has_tool_calls: bool,
    sa_mode: bool,
) -> PostExecContext<'a> {
    PostExecContext {
        executor,
        prompt: "test prompt",
        effective_prompt: "test prompt",
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
        timeout_diagnostics: None,
        has_tool_calls,
        turn_count: 0,
        output_tokens: None,
        sa_mode,
    }
}

fn build_test_result(summary: &str) -> csa_process::ExecutionResult {
    csa_process::ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: summary.to_string(),
        exit_code: 0,
        peak_memory_mb: None,
        ..Default::default()
    }
}

fn build_failed_test_result(summary: &str, stderr: &str) -> csa_process::ExecutionResult {
    csa_process::ExecutionResult {
        output: String::new(),
        stderr_output: stderr.to_string(),
        summary: summary.to_string(),
        exit_code: 1,
        peak_memory_mb: None,
        ..Default::default()
    }
}

fn write_result_sidecar(session_dir: &std::path::Path, contents: &str) {
    let path = session_dir.join(csa_session::CONTRACT_RESULT_ARTIFACT_PATH);
    std::fs::create_dir_all(path.parent().expect("result sidecar parent"))
        .expect("create output dir");
    std::fs::write(path, contents).expect("write output/result.toml");
}

#[path = "pipeline_tests_no_op_gate_task_type.rs"]
mod task_type_tests;

#[tokio::test]
async fn permanent_tool_quota_sets_tool_exhausted_phase() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    let mut session =
        create_session(project_root, Some("test"), None, Some("gemini-cli")).expect("create");
    let session_dir =
        csa_session::get_session_dir(project_root, &session.meta_session_id).expect("dir");

    let executor = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };
    let hooks_config = csa_hooks::HooksConfig::default();
    let start = chrono::Utc::now() - chrono::Duration::seconds(15);
    let ctx = build_test_ctx(
        &executor,
        session_dir,
        project_root,
        start,
        &hooks_config,
        false,
        true,
    );
    let mut result = build_failed_test_result(
        "gemini failed",
        "status: RESOURCE_EXHAUSTED; reason: QUOTA_EXHAUSTED; monthly spending cap reached",
    );

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process_execution_result");

    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load")
        .expect("result exists");
    assert_eq!(persisted.exit_code, 1);
    assert_eq!(persisted.status, SessionResult::status_from_exit_code(1));
    assert!(persisted.summary.starts_with("tool_exhausted: gemini-cli"));
    let reloaded = load_session(project_root, &session.meta_session_id).expect("load session");
    assert_eq!(reloaded.phase, SessionPhase::ToolExhausted);
    assert_eq!(
        reloaded.termination_reason.as_deref(),
        Some("tool_exhausted")
    );
}

#[tokio::test]
async fn no_op_gate_triggers_when_sa_mode_and_zero_tools_and_short_elapsed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    let mut session =
        create_session(project_root, Some("test"), None, Some("claude-code")).expect("create");
    let session_dir =
        csa_session::get_session_dir(project_root, &session.meta_session_id).expect("dir");

    let executor = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: ClaudeCodeRuntimeMetadata::current(),
    };
    let hooks_config = csa_hooks::HooksConfig::default();
    let start = chrono::Utc::now() - chrono::Duration::seconds(15);
    let ctx = build_test_ctx(
        &executor,
        session_dir,
        project_root,
        start,
        &hooks_config,
        false,
        true,
    );
    let mut result = build_test_result("I'll start by exploring the codebase.");

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process_execution_result");

    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load")
        .expect("result exists");
    assert_eq!(persisted.exit_code, 1);
    assert_eq!(persisted.status, SessionResult::status_from_exit_code(1));
    assert!(
        persisted.summary.starts_with("no-op exit detected"),
        "summary should start with diagnostic prefix, got: {}",
        persisted.summary
    );
    assert_eq!(
        result.exit_code, 1,
        "ExecutionResult exit_code must also be rewritten"
    );
}

#[tokio::test]
async fn no_op_gate_preserves_success_when_result_sidecar_reports_success() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    let mut session =
        create_session(project_root, Some("test"), None, Some("claude-code")).expect("create");
    let session_dir =
        csa_session::get_session_dir(project_root, &session.meta_session_id).expect("dir");
    write_result_sidecar(
        &session_dir,
        r#"[result]
status = "success"
summary = "PASS"
"#,
    );

    let executor = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: ClaudeCodeRuntimeMetadata::current(),
    };
    let hooks_config = csa_hooks::HooksConfig::default();
    let start = chrono::Utc::now() - chrono::Duration::seconds(15);
    let ctx = build_test_ctx(
        &executor,
        session_dir,
        project_root,
        start,
        &hooks_config,
        false,
        true,
    );
    let mut result = build_test_result("PASS");

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process_execution_result");

    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load")
        .expect("result exists");
    assert_eq!(persisted.exit_code, 0);
    assert_eq!(persisted.status, SessionResult::status_from_exit_code(0));
    assert!(
        !persisted.summary.starts_with("no-op exit detected"),
        "success sidecar must suppress no-op rewrite, got: {}",
        persisted.summary
    );
    assert_eq!(result.exit_code, 0);
}

#[tokio::test]
async fn no_op_gate_still_triggers_when_result_sidecar_reports_failure() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    let mut session =
        create_session(project_root, Some("test"), None, Some("claude-code")).expect("create");
    let session_dir =
        csa_session::get_session_dir(project_root, &session.meta_session_id).expect("dir");
    write_result_sidecar(
        &session_dir,
        r#"[result]
status = "failure"
summary = "not done"
"#,
    );

    let executor = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: ClaudeCodeRuntimeMetadata::current(),
    };
    let hooks_config = csa_hooks::HooksConfig::default();
    let start = chrono::Utc::now() - chrono::Duration::seconds(15);
    let ctx = build_test_ctx(
        &executor,
        session_dir,
        project_root,
        start,
        &hooks_config,
        false,
        true,
    );
    let mut result = build_test_result("I'll start by exploring the codebase.");

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process_execution_result");

    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load")
        .expect("result exists");
    assert_eq!(persisted.exit_code, 1);
    assert_eq!(persisted.status, SessionResult::status_from_exit_code(1));
    assert!(
        persisted.summary.starts_with("no-op exit detected"),
        "failure sidecar must not suppress no-op rewrite, got: {}",
        persisted.summary
    );
}

#[tokio::test]
async fn no_op_gate_does_not_trigger_without_sa_mode() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    let mut session =
        create_session(project_root, Some("test"), None, Some("claude-code")).expect("create");
    let session_dir =
        csa_session::get_session_dir(project_root, &session.meta_session_id).expect("dir");

    let executor = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: ClaudeCodeRuntimeMetadata::current(),
    };
    let hooks_config = csa_hooks::HooksConfig::default();
    let start = chrono::Utc::now() - chrono::Duration::seconds(15);
    let ctx = build_test_ctx(
        &executor,
        session_dir,
        project_root,
        start,
        &hooks_config,
        false,
        false,
    );
    let mut result = build_test_result("I'll start by exploring the codebase.");

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process_execution_result");

    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load")
        .expect("result exists");
    assert_eq!(persisted.exit_code, 0);
    assert_eq!(persisted.status, SessionResult::status_from_exit_code(0));
    assert!(
        !persisted.summary.starts_with("no-op exit detected"),
        "summary must NOT be prefixed when sa_mode=false"
    );
}

#[tokio::test]
async fn no_op_gate_does_not_trigger_when_tool_calls_observed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    let mut session =
        create_session(project_root, Some("test"), None, Some("claude-code")).expect("create");
    let session_dir =
        csa_session::get_session_dir(project_root, &session.meta_session_id).expect("dir");

    let executor = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: ClaudeCodeRuntimeMetadata::current(),
    };
    let hooks_config = csa_hooks::HooksConfig::default();
    let start = chrono::Utc::now() - chrono::Duration::seconds(15);
    let ctx = build_test_ctx(
        &executor,
        session_dir,
        project_root,
        start,
        &hooks_config,
        true,
        true,
    );
    let mut result = build_test_result("Ran tools and explored.");

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process_execution_result");

    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load")
        .expect("result exists");
    assert_eq!(persisted.exit_code, 0);
    assert_eq!(persisted.status, SessionResult::status_from_exit_code(0));
}

#[tokio::test]
async fn no_op_gate_does_not_trigger_when_elapsed_exceeds_threshold() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    let mut session =
        create_session(project_root, Some("test"), None, Some("claude-code")).expect("create");
    let session_dir =
        csa_session::get_session_dir(project_root, &session.meta_session_id).expect("dir");

    let executor = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: ClaudeCodeRuntimeMetadata::current(),
    };
    let hooks_config = csa_hooks::HooksConfig::default();
    // 65 seconds elapsed — exceeds the 60s threshold
    let start = chrono::Utc::now() - chrono::Duration::seconds(65);
    let ctx = build_test_ctx(
        &executor,
        session_dir,
        project_root,
        start,
        &hooks_config,
        false,
        true,
    );
    let mut result = build_test_result("Spent time thinking.");

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process_execution_result");

    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load")
        .expect("result exists");
    assert_eq!(persisted.exit_code, 0);
    assert_eq!(persisted.status, SessionResult::status_from_exit_code(0));
}

#[tokio::test]
async fn no_op_gate_does_not_trigger_with_high_output_tokens() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    let mut session =
        create_session(project_root, Some("test"), None, Some("claude-code")).expect("create");
    let session_dir =
        csa_session::get_session_dir(project_root, &session.meta_session_id).expect("dir");

    let executor = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: ClaudeCodeRuntimeMetadata::current(),
    };
    let hooks_config = csa_hooks::HooksConfig::default();
    let start = chrono::Utc::now() - chrono::Duration::seconds(15);
    let mut ctx = build_test_ctx(
        &executor,
        session_dir,
        project_root,
        start,
        &hooks_config,
        false,
        true,
    );
    ctx.output_tokens = Some(14_093);
    let mut result = build_test_result("Security audit completed with a PASS verdict.");

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process_execution_result");

    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load")
        .expect("result exists");
    assert_eq!(persisted.exit_code, 0);
    assert_eq!(persisted.status, SessionResult::status_from_exit_code(0));
    assert!(
        !persisted.summary.starts_with("no-op exit detected"),
        "high output_tokens must suppress no-op rewrite, got: {}",
        persisted.summary
    );
    assert_eq!(result.exit_code, 0);
}

#[tokio::test]
async fn no_op_gate_preserves_original_summary_as_suffix() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    let mut session =
        create_session(project_root, Some("test"), None, Some("claude-code")).expect("create");
    let session_dir =
        csa_session::get_session_dir(project_root, &session.meta_session_id).expect("dir");

    let executor = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: ClaudeCodeRuntimeMetadata::current(),
    };
    let hooks_config = csa_hooks::HooksConfig::default();
    let start = chrono::Utc::now() - chrono::Duration::seconds(10);
    let original_summary = "I'll start by exploring the codebase structure.";
    let ctx = build_test_ctx(
        &executor,
        session_dir,
        project_root,
        start,
        &hooks_config,
        false,
        true,
    );
    let mut result = build_test_result(original_summary);

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process_execution_result");

    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load")
        .expect("result exists");
    assert!(
        persisted.summary.starts_with("no-op exit detected"),
        "should start with diagnostic, got: {}",
        persisted.summary
    );
    assert!(
        persisted.summary.contains(original_summary),
        "original summary must be preserved as suffix, got: {}",
        persisted.summary
    );
}

#[tokio::test]
async fn no_op_gate_does_not_trigger_when_changed_paths_present() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    let mut session =
        create_session(project_root, Some("test"), None, Some("claude-code")).expect("create");
    let session_dir =
        csa_session::get_session_dir(project_root, &session.meta_session_id).expect("dir");

    let executor = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: ClaudeCodeRuntimeMetadata::current(),
    };
    let hooks_config = csa_hooks::HooksConfig::default();
    let start = chrono::Utc::now() - chrono::Duration::seconds(15);
    let mut ctx = build_test_ctx(
        &executor,
        session_dir,
        project_root,
        start,
        &hooks_config,
        false,
        true,
    );
    ctx.changed_paths = vec!["src/foo.rs".to_string()];
    let mut result = build_test_result("Applied the fix.");

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process_execution_result");

    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load")
        .expect("result exists");
    assert_eq!(persisted.exit_code, 0);
    assert_eq!(persisted.status, SessionResult::status_from_exit_code(0));
    assert!(
        !persisted.summary.starts_with("no-op exit detected"),
        "gate must not fire when changed_paths is non-empty, got: {}",
        persisted.summary
    );
}

#[tokio::test]
async fn no_op_gate_syncs_tool_state_last_exit_code_and_summary() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    let mut session =
        create_session(project_root, Some("test"), None, Some("claude-code")).expect("create");
    // Pre-seed the tool state entry so we can verify it gets overwritten.
    session.tools.insert(
        "claude-code".to_string(),
        csa_session::ToolState {
            provider_session_id: None,
            last_action_summary: "original".to_string(),
            last_exit_code: 0,
            updated_at: chrono::Utc::now(),
            tool_version: None,
            token_usage: None,
        },
    );
    let session_dir =
        csa_session::get_session_dir(project_root, &session.meta_session_id).expect("dir");

    let executor = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: ClaudeCodeRuntimeMetadata::current(),
    };
    let hooks_config = csa_hooks::HooksConfig::default();
    let start = chrono::Utc::now() - chrono::Duration::seconds(10);
    let ctx = build_test_ctx(
        &executor,
        session_dir,
        project_root,
        start,
        &hooks_config,
        false,
        true,
    );
    let mut result = build_test_result("I'll start by exploring the codebase.");

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process_execution_result");

    let tool_state = session
        .tools
        .get("claude-code")
        .expect("tool state must exist after process_execution_result");
    assert_eq!(
        tool_state.last_exit_code, 1,
        "tool_state.last_exit_code must be synced to 1 after gate fires"
    );
    assert!(
        tool_state
            .last_action_summary
            .starts_with("no-op exit detected"),
        "tool_state.last_action_summary must reflect gate rewrite, got: {}",
        tool_state.last_action_summary
    );
}

#[tokio::test]
async fn no_op_gate_does_not_trigger_when_turn_count_exceeds_one() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    let mut session =
        create_session(project_root, Some("test"), None, Some("claude-code")).expect("create");
    // Simulate a session that already had 5 turns before this execution.
    // process_execution_result increments turn_count, so 5 → 6 which is > 1.
    session.turn_count = 5;
    let session_dir =
        csa_session::get_session_dir(project_root, &session.meta_session_id).expect("dir");

    let executor = Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: ClaudeCodeRuntimeMetadata::current(),
    };
    let hooks_config = csa_hooks::HooksConfig::default();
    let start = chrono::Utc::now() - chrono::Duration::seconds(15);
    let ctx = build_test_ctx(
        &executor,
        session_dir,
        project_root,
        start,
        &hooks_config,
        false,
        true,
    );
    let mut result = build_test_result("Quick response.");

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process_execution_result");

    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load")
        .expect("result exists");
    assert_eq!(persisted.exit_code, 0);
    assert_eq!(persisted.status, SessionResult::status_from_exit_code(0));
    assert!(
        !persisted.summary.starts_with("no-op exit detected"),
        "gate must not fire for multi-turn sessions"
    );
}
