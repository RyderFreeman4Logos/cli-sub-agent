use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_session::{create_session, get_session_dir, load_session};
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::Command;

#[path = "pipeline_tests_post_exec_signal.rs"]
mod signal;

#[test]
fn dirty_sa_needs_receipt() {
    let paths = vec![String::new()];
    assert!(dirty_sa_run_lacks_completion_receipt(
        true,
        Some("run"),
        &paths,
        false
    ));
    assert!(!dirty_sa_run_lacks_completion_receipt(
        true,
        Some("run"),
        &paths,
        true
    ));
}

#[test]
fn ensure_terminal_result_on_post_exec_error_writes_missing_result() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project_root = tmp.path();
    let mut session =
        create_session(project_root, Some("test"), None, Some("codex")).expect("create session");

    assert!(
        load_result(project_root, &session.meta_session_id)
            .expect("load result")
            .is_none(),
        "precondition: result.toml must be missing"
    );

    let started_at = chrono::Utc::now() - chrono::Duration::seconds(1);
    let err = anyhow::anyhow!("post-run hook failed");
    ensure_terminal_result_on_post_exec_error(
        project_root,
        &mut session,
        "codex",
        started_at,
        &err,
    );

    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load fallback result")
        .expect("fallback result should exist");
    assert_eq!(persisted.status, "failure");
    assert_eq!(persisted.exit_code, 1);
    assert!(
        persisted.summary.contains("post-exec:"),
        "summary should indicate post-exec fallback"
    );

    let reloaded = load_session(project_root, &session.meta_session_id)
        .expect("reload session after fallback");
    assert_eq!(
        reloaded.termination_reason.as_deref(),
        Some("post_exec_error")
    );
    assert_eq!(reloaded.phase, SessionPhase::Retired);
}

#[test]
fn ensure_terminal_result_on_post_exec_error_keeps_existing_result() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project_root = tmp.path();
    let mut session =
        create_session(project_root, Some("test"), None, Some("codex")).expect("create session");
    let now = chrono::Utc::now();
    let existing = SessionResult {
        post_exec_gate: None,
        status: "success".to_string(),
        exit_code: 0,
        summary: "already persisted".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 1,
        artifacts: vec![SessionArtifact::new("output/acp-events.jsonl")],
        ..Default::default()
    };
    save_result(project_root, &session.meta_session_id, &existing).expect("write existing result");

    let err = anyhow::anyhow!("late hook failure");
    ensure_terminal_result_on_post_exec_error(project_root, &mut session, "codex", now, &err);

    let persisted = load_result(project_root, &session.meta_session_id)
        .expect("load existing result")
        .expect("existing result should remain");
    assert_eq!(persisted.status, "success");
    assert_eq!(persisted.exit_code, 0);
    assert_eq!(persisted.summary, "already persisted");

    let reloaded = load_session(project_root, &session.meta_session_id)
        .expect("reload session after fallback");
    assert_eq!(reloaded.phase, SessionPhase::Retired);
    assert!(
        reloaded.termination_reason.is_none(),
        "finalizer fallback must not mark a worker-success session as a failure"
    );
}

#[test]
fn ensure_terminal_result_for_session_on_post_exec_error_persists_output_tail_for_fork() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project_root = tmp.path();
    let parent = create_session(project_root, Some("parent"), None, Some("codex"))
        .expect("create parent session");
    let child = create_session(
        project_root,
        Some("fork"),
        Some(&parent.meta_session_id),
        Some("codex"),
    )
    .expect("create forked child session");
    let session_id = child.meta_session_id.clone();
    let session_dir = get_session_dir(project_root, &session_id).expect("session dir");
    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output.log"),
        "first line\nstill running\npartial summary line\n",
    )
    .expect("write output log");
    fs::write(
        session_dir.join("output").join("user-result.toml"),
        "status = \"success\"\nsummary = \"sidecar\"\n",
    )
    .expect("write sidecar result");

    let started_at = chrono::Utc::now() - chrono::Duration::seconds(1);
    let err = anyhow::anyhow!("wall timeout interrupted fork before post-exec");
    ensure_terminal_result_for_session_on_post_exec_error(
        project_root,
        &session_id,
        "codex",
        started_at,
        &err,
    );

    let persisted = load_result(project_root, &session_id)
        .expect("load fallback result")
        .expect("fallback result should exist");
    assert_eq!(persisted.status, "failure");
    assert_eq!(persisted.exit_code, 1);
    assert!(
        persisted.summary.contains("partial summary line"),
        "summary should include output.log tail"
    );
    assert!(
        persisted
            .artifacts
            .iter()
            .any(|artifact| artifact.path == "output/user-result.toml"),
        "fallback should register user-result sidecar"
    );

    let reloaded = load_session(project_root, &session_id).expect("reload session");
    assert_eq!(
        reloaded.termination_reason.as_deref(),
        Some("post_exec_error")
    );
    assert_eq!(reloaded.phase, SessionPhase::Retired);
}

#[path = "pipeline_tests_post_exec_jj.rs"]
mod jj;

// Handoff artifact tests are in pipeline_handoff.rs

#[test]
fn codex_exec_initial_stall_summary_forces_failure_status_in_result_toml() {
    let now = chrono::Utc::now();
    let mut result = SessionResult {
        post_exec_gate: None,
        status: SessionResult::status_from_exit_code(137),
        exit_code: 137,
        summary: "codex_exec_initial_stall: no stdout within 300s (effort=high, retry_attempted=true, original_effort=xhigh)".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: Vec::new(),
        ..Default::default()
    };

    if is_codex_exec_initial_stall_summary(&result.tool, result.exit_code, &result.summary) {
        result.status = SessionResult::status_from_exit_code(1);
    }

    let toml = toml::to_string_pretty(&result).expect("serialize result.toml");
    assert_eq!(result.status, "failure");
    assert!(toml.contains("status = \"failure\""));
    assert!(toml.contains(CODEX_EXEC_INITIAL_STALL_REASON));
}

#[test]
fn codex_exec_initial_stall_detection_rejects_plain_substring_collisions() {
    assert!(!is_codex_exec_initial_stall_summary(
        "codex",
        0,
        "completed successfully after discussing codex_exec_initial_stall handling"
    ));
    assert!(!is_codex_exec_initial_stall_summary(
        "claude-code",
        137,
        "codex_exec_initial_stall: no stdout within 300s (effort=high, retry_attempted=true)"
    ));
}

#[test]
fn read_output_log_tail_reads_from_file_end_window() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let session_dir = tmp.path();
    let contents = (0..1500)
        .map(|idx| format!("line-{idx:04}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(session_dir.join("output.log"), format!("{contents}\n")).expect("write output");

    let tail = read_output_log_tail(session_dir, 3).expect("tail");
    assert_eq!(tail, "line-1497\nline-1498\nline-1499");
    assert!(
        !tail.contains("line-0000"),
        "tail reader should not depend on loading the full file"
    );
}

struct CurrentDirGuard {
    original: std::path::PathBuf,
}

impl CurrentDirGuard {
    fn enter(path: &Path) -> Self {
        let original = std::env::current_dir().expect("current dir");
        std::env::set_current_dir(path).expect("set current dir");
        Self { original }
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.original).expect("restore current dir");
    }
}

fn write_executable_script(path: &Path, body: &str) {
    let mut script = fs::File::create(path).expect("create script");
    write!(script, "{body}").expect("write script");
    script.sync_all().expect("sync script");
    drop(script);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod script");
    }
}

#[tokio::test]
async fn process_execution_result_mempal_payload_uses_target_project_cwd() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut sandbox = ScopedSessionSandbox::new(&temp).await;
    sandbox.track_env("PATH");

    let invocation_cwd = temp.path().join("cli-sub-agent-install");
    let project_root = temp.path().join("warifu-ce");
    fs::create_dir_all(&invocation_cwd).expect("create invocation cwd");
    fs::create_dir_all(&project_root).expect("create project root");
    let _cwd = CurrentDirGuard::enter(&invocation_cwd);

    let fake_bin = temp.path().join("bin");
    fs::create_dir_all(&fake_bin).expect("create fake bin");
    let payload_path = temp.path().join("mempal-payload.json");
    write_executable_script(
        &fake_bin.join("mempal"),
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'mempal mock 0.0.0\\n'\n  exit 0\nfi\nif [ \"$1\" = \"ingest\" ] && [ \"$2\" = \"--stdin\" ] && [ \"$3\" = \"--json\" ]; then\n  cat > '{}'\n  exit 0\nfi\nexit 64\n",
            payload_path.display()
        ),
    );
    let original_path = std::env::var_os("PATH").unwrap_or_default();
    let mut path_entries = vec![fake_bin.clone()];
    path_entries.extend(std::env::split_paths(&original_path));
    let joined_path = std::env::join_paths(path_entries).expect("join PATH");
    // SAFETY: ScopedSessionSandbox holds TEST_ENV_LOCK for this test.
    unsafe { std::env::set_var("PATH", joined_path) };

    let executor = csa_executor::Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: csa_executor::codex_runtime::codex_runtime_metadata(),
    };
    let config: csa_config::ProjectConfig = toml::from_str(
        r#"
schema_version = 1

[memory]
backend = "mempal"
auto_capture = true
"#,
    )
    .expect("project config");
    let hooks_config = csa_hooks::HooksConfig::default();
    let mut session =
        create_session(&project_root, Some("target cwd"), None, Some("codex")).expect("session");
    let session_dir =
        get_session_dir(&project_root, &session.meta_session_id).expect("resolve session dir");

    let ctx = PostExecContext {
        executor: &executor,
        prompt: "test prompt",
        effective_prompt: "test prompt",
        task_type: Some("run"),
        readonly_project_root: false,
        project_root: &project_root,
        config: Some(&config),
        global_config: None,
        session_dir,
        sessions_root: "test-root".to_string(),
        execution_start_time: chrono::Utc::now() - chrono::Duration::seconds(1),
        hooks_config: &hooks_config,
        memory_project_key: None,
        provider_session_id: None,
        events_count: 1,
        transcript_artifacts: vec![],
        changed_paths: vec![],
        pre_exec_snapshot: None,
        timeout_diagnostics: None,
        has_tool_calls: true,
        turn_count: 0,
        output_tokens: None,
        sa_mode: false,
        original_exit_code: None,
    };
    let mut result = csa_process::ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "captured via session complete".to_string(),
        exit_code: 0,
        peak_memory_mb: None,
        ..Default::default()
    };

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process result");

    for _ in 0..50 {
        if payload_path.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(payload_path.exists(), "mempal payload should be written");

    let payload: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&payload_path).expect("read payload"))
            .expect("parse payload");
    assert_eq!(payload["project"], "warifu-ce");
    assert_eq!(payload["cwd"], project_root.display().to_string());
    assert_eq!(payload["claude_cwd"], project_root.display().to_string());
    assert_ne!(
        payload["cwd"],
        invocation_cwd.display().to_string(),
        "session mempal payload must use target project root, not process cwd"
    );
}

// --- R10-F3: dirty-SA exit preservation vs contract coercion ---

#[test]
fn r10_dirty_sa_preserves_original_exit_when_contract_coerced_to_one() {
    // R10-F3: result-contract enforcement runs BEFORE dirty-SA preservation and
    // may call `mark_gate_failure`, coercing `exit_code` to 1. The dirty-SA
    // path must preserve the ORIGINAL nonzero exit (e.g. timeout 124), not the
    // contract's generic 1.
    use csa_session::create_session;
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project_root = tmp.path();
    let mut session =
        create_session(project_root, Some("test"), None, Some("codex")).expect("create session");
    let hooks_config = csa_hooks::HooksConfig::default();
    let executor = csa_executor::Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: csa_executor::codex_runtime::codex_runtime_metadata(),
    };
    let session_dir =
        get_session_dir(project_root, &session.meta_session_id).expect("resolve session dir");
    // Simulate a dirty SA-mode run: changed paths present, sa_mode on, task=run.
    let ctx = crate::pipeline_post_exec::PostExecContext {
        executor: &executor,
        prompt: "write src.rs",
        effective_prompt: "write src.rs",
        task_type: Some("run"),
        readonly_project_root: false,
        project_root,
        config: None,
        global_config: None,
        session_dir,
        sessions_root: "test-root".to_string(),
        execution_start_time: chrono::Utc::now() - chrono::Duration::seconds(1),
        hooks_config: &hooks_config,
        memory_project_key: None,
        provider_session_id: None,
        events_count: 1,
        transcript_artifacts: vec![],
        changed_paths: vec!["src.rs".to_string()],
        pre_exec_snapshot: None,
        timeout_diagnostics: None,
        has_tool_calls: true,
        turn_count: 1,
        output_tokens: None,
        sa_mode: true,
        // The ORIGINAL exit (timeout) captured pre-contract.
        original_exit_code: Some(124),
    };
    // Contract already coerced exit_code to 1 via mark_gate_failure.
    let mut result = csa_process::ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "contract violation: missing result.toml".to_string(),
        exit_code: 1,
        peak_memory_mb: None,
        csa_gate_failure: Some("result-toml-contract".to_string()),
        ..Default::default()
    };
    let mut session_result = csa_session::SessionResult {
        post_exec_gate: None,
        status: csa_session::SessionResult::status_from_exit_code(124),
        exit_code: 124,
        summary: "timed out".to_string(),
        tool: "codex".to_string(),
        ..Default::default()
    };
    maybe_mark_dirty_sa_run_without_receipt(
        &ctx,
        &mut session,
        &mut result,
        &mut session_result,
        false,
    );
    // The dirty-SA path must preserve the ORIGINAL 124, not the coerced 1.
    assert_eq!(
        result.exit_code, 124,
        "dirty-SA must preserve original timeout exit (124), not contract-coerced 1"
    );
    assert_eq!(
        session_result.exit_code, 124,
        "session_result must preserve original timeout exit (124)"
    );
    assert_eq!(
        result.csa_gate_failure.as_deref(),
        Some("result-toml-contract"),
        "dirty-SA preserves the first canonical gate-failure reason (contract)"
    );
}
