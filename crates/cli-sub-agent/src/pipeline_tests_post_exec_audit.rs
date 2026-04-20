use super::{
    apply_repo_write_audit_to_result, maybe_record_repo_write_audit, pre_execution_audit_baseline,
    should_audit_repo_tracked_writes,
};
use crate::pipeline_post_exec::{PostExecContext, PreExecutionSnapshot};
use csa_config::GlobalConfig;
use csa_executor::{CodexRuntimeMetadata, Executor};
use csa_session::{MetaSessionState, RepoWriteAudit, SessionResult};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;

#[test]
fn should_audit_repo_tracked_writes_for_explicit_readonly_run() {
    assert!(should_audit_repo_tracked_writes(
        Some("run"),
        false,
        "Read-only: inspect src/main.rs and summarize what it does"
    ));
}

#[test]
fn should_audit_repo_tracked_writes_for_recon_style_run() {
    assert!(should_audit_repo_tracked_writes(
        Some("run"),
        false,
        "Analyze the main module and summarize the control flow"
    ));
}

#[test]
fn should_not_audit_repo_tracked_writes_for_mutating_run() {
    assert!(!should_audit_repo_tracked_writes(
        Some("run"),
        false,
        "Implement the fix in src/main.rs and update tests"
    ));
}

#[test]
fn should_audit_repo_tracked_writes_for_plan_task_type() {
    assert!(should_audit_repo_tracked_writes(
        Some("plan"),
        false,
        "Analyze the workflow and summarize where files are written"
    ));
}

#[test]
fn should_audit_repo_tracked_writes_for_plan_step_task_type() {
    assert!(should_audit_repo_tracked_writes(
        Some("plan-step"),
        false,
        "Read-only: inspect the task step and summarize the result"
    ));
}

#[test]
fn should_not_audit_repo_tracked_writes_for_review_or_debate() {
    assert!(!should_audit_repo_tracked_writes(
        Some("review"),
        true,
        "Analyze the diff and summarize findings"
    ));
    assert!(!should_audit_repo_tracked_writes(
        Some("debate"),
        true,
        "Analyze the proposal and summarize tradeoffs"
    ));
}

#[test]
fn should_not_audit_repo_tracked_writes_for_unknown_task_type() {
    assert!(!should_audit_repo_tracked_writes(
        None,
        true,
        "Analyze the module and summarize the control flow"
    ));
}

#[test]
fn apply_repo_write_audit_to_result_populates_manager_sidecar_sections() {
    let mut session_result = SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "ok".to_string(),
        tool: "codex".to_string(),
        started_at: chrono::Utc::now(),
        completed_at: chrono::Utc::now(),
        events_count: 0,
        artifacts: vec![],
        peak_memory_mb: None,
        manager_fields: Default::default(),
    };
    let audit = RepoWriteAudit {
        added: vec![PathBuf::from("new.txt")],
        modified: vec![PathBuf::from("tracked.txt")],
        deleted: vec![PathBuf::from("old.txt")],
        renamed: vec![(PathBuf::from("src/a.rs"), PathBuf::from("src/b.rs"))],
    };

    apply_repo_write_audit_to_result(&mut session_result, &audit).unwrap();

    let repo_write_audit = session_result
        .manager_fields
        .artifacts
        .as_ref()
        .and_then(|value| value.get("repo_write_audit"))
        .expect("repo write audit sidecar");
    assert_eq!(
        repo_write_audit
            .get("added")
            .and_then(toml::Value::as_array),
        Some(&vec![toml::Value::String("new.txt".to_string())])
    );
    assert_eq!(
        repo_write_audit
            .get("modified")
            .and_then(toml::Value::as_array),
        Some(&vec![toml::Value::String("tracked.txt".to_string())])
    );
    assert_eq!(
        repo_write_audit
            .get("deleted")
            .and_then(toml::Value::as_array),
        Some(&vec![toml::Value::String("old.txt".to_string())])
    );
    let renamed = repo_write_audit
        .get("renamed")
        .and_then(toml::Value::as_array)
        .expect("renamed entries");
    assert_eq!(renamed.len(), 1);
    assert_eq!(
        renamed[0].get("from"),
        Some(&toml::Value::String("src/a.rs".to_string()))
    );
    assert_eq!(
        renamed[0].get("to"),
        Some(&toml::Value::String("src/b.rs".to_string()))
    );
}

#[test]
fn pre_execution_audit_baseline_returns_none_for_legacy_sessions_without_snapshot() {
    let session = MetaSessionState {
        meta_session_id: "01TESTLEGACYAUDIT0000000000".to_string(),
        description: None,
        project_path: "/tmp/project".to_string(),
        branch: None,
        created_at: chrono::Utc::now(),
        last_accessed: chrono::Utc::now(),
        genealogy: Default::default(),
        tools: Default::default(),
        context_status: Default::default(),
        total_token_usage: None,
        phase: Default::default(),
        task_context: Default::default(),
        turn_count: 0,
        token_budget: None,
        sandbox_info: None,
        termination_reason: None,
        is_seed_candidate: false,
        git_head_at_creation: None,
        pre_session_porcelain: None,
        last_return_packet: None,
        change_id: None,
        spec_id: None,
        vcs_identity: None,
        identity_version: 2,
        fork_call_timestamps: Vec::new(),
    };

    let executor = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: CodexRuntimeMetadata::current(),
    };
    let global_config = GlobalConfig::default();
    let tempdir = tempfile::tempdir().unwrap();
    let ctx = PostExecContext {
        executor: &executor,
        prompt: "Read-only: inspect tracked.txt and summarize changes",
        effective_prompt: "Read-only: inspect tracked.txt and summarize changes",
        task_type: Some("run"),
        readonly_project_root: true,
        project_root: tempdir.path(),
        config: None,
        global_config: Some(&global_config),
        session_dir: tempdir.path().join("session"),
        sessions_root: "test-root".to_string(),
        execution_start_time: chrono::Utc::now(),
        hooks_config: &csa_hooks::HooksConfig::default(),
        memory_project_key: None,
        provider_session_id: None,
        events_count: 0,
        transcript_artifacts: vec![],
        changed_paths: vec![],
        pre_exec_snapshot: None,
    };

    assert_eq!(pre_execution_audit_baseline(&ctx, &session), None);
}

#[test]
fn pre_execution_audit_baseline_prefers_per_execution_snapshot() {
    let session = MetaSessionState {
        meta_session_id: "01TESTAUDITBASELINE000000000".to_string(),
        description: None,
        project_path: "/tmp/project".to_string(),
        branch: None,
        created_at: chrono::Utc::now(),
        last_accessed: chrono::Utc::now(),
        genealogy: Default::default(),
        tools: Default::default(),
        context_status: Default::default(),
        total_token_usage: None,
        phase: Default::default(),
        task_context: Default::default(),
        turn_count: 0,
        token_budget: None,
        sandbox_info: None,
        termination_reason: None,
        is_seed_candidate: false,
        git_head_at_creation: Some("abc123".to_string()),
        pre_session_porcelain: Some(" M tracked.txt\0".to_string()),
        last_return_packet: None,
        change_id: None,
        spec_id: None,
        vcs_identity: None,
        identity_version: 2,
        fork_call_timestamps: Vec::new(),
    };
    let executor = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: CodexRuntimeMetadata::current(),
    };
    let global_config = GlobalConfig::default();
    let tempdir = tempfile::tempdir().unwrap();
    let ctx = PostExecContext {
        executor: &executor,
        prompt: "Read-only: inspect tracked.txt and summarize changes",
        effective_prompt: "Read-only: inspect tracked.txt and summarize changes",
        task_type: Some("run"),
        readonly_project_root: true,
        project_root: tempdir.path(),
        config: None,
        global_config: Some(&global_config),
        session_dir: tempdir.path().join("session"),
        sessions_root: "test-root".to_string(),
        execution_start_time: chrono::Utc::now(),
        hooks_config: &csa_hooks::HooksConfig::default(),
        memory_project_key: None,
        provider_session_id: None,
        events_count: 0,
        transcript_artifacts: vec![],
        changed_paths: vec![],
        pre_exec_snapshot: Some(PreExecutionSnapshot {
            head: "def456".to_string(),
            porcelain: Some(" M fresh.txt\0".to_string()),
        }),
    };

    assert_eq!(
        pre_execution_audit_baseline(&ctx, &session),
        Some(("def456", Some(" M fresh.txt\0")))
    );
}

#[derive(Clone, Default)]
struct SharedLogBuffer {
    inner: Arc<Mutex<Vec<u8>>>,
}

impl SharedLogBuffer {
    fn contents(&self) -> String {
        String::from_utf8(self.inner.lock().expect("log buffer poisoned").clone())
            .expect("log buffer should be valid UTF-8")
    }
}

struct BufferWriter {
    inner: Arc<Mutex<Vec<u8>>>,
}

impl Write for BufferWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner
            .lock()
            .expect("log buffer poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for SharedLogBuffer {
    type Writer = BufferWriter;

    fn make_writer(&'a self) -> Self::Writer {
        BufferWriter {
            inner: Arc::clone(&self.inner),
        }
    }
}

#[test]
fn audit_failure_does_not_fail_execution() {
    let repo = tempfile::tempdir().unwrap();
    run_git(repo.path(), &["init"]);
    run_git(repo.path(), &["config", "user.email", "test@example.com"]);
    run_git(repo.path(), &["config", "user.name", "Test User"]);
    std::fs::write(repo.path().join("tracked.txt"), "before\n").unwrap();
    run_git(repo.path(), &["add", "tracked.txt"]);
    run_git(repo.path(), &["commit", "-m", "init"]);

    let pre_head = detect_git_head(repo.path()).unwrap();
    let pre_porcelain = git_status_porcelain(repo.path());

    std::fs::write(repo.path().join("tracked.txt"), "after\n").unwrap();

    let session_dir_file = repo.path().join("not-a-session-dir");
    std::fs::write(&session_dir_file, "blocking file\n").unwrap();

    let executor = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: CodexRuntimeMetadata::current(),
    };
    let global_config = GlobalConfig::default();
    let ctx = PostExecContext {
        executor: &executor,
        prompt: "Read-only: inspect tracked.txt and summarize changes",
        effective_prompt: "Read-only: inspect tracked.txt and summarize changes",
        task_type: Some("run"),
        readonly_project_root: true,
        project_root: repo.path(),
        config: None,
        global_config: Some(&global_config),
        session_dir: session_dir_file.clone(),
        sessions_root: "test-root".to_string(),
        execution_start_time: chrono::Utc::now(),
        hooks_config: &csa_hooks::HooksConfig::default(),
        memory_project_key: None,
        provider_session_id: None,
        events_count: 0,
        transcript_artifacts: vec![],
        changed_paths: vec![],
        pre_exec_snapshot: None,
    };
    let session = MetaSessionState {
        meta_session_id: "01TESTAUDITFAILURE000000000".to_string(),
        description: None,
        project_path: repo.path().display().to_string(),
        branch: None,
        created_at: chrono::Utc::now(),
        last_accessed: chrono::Utc::now(),
        genealogy: Default::default(),
        tools: Default::default(),
        context_status: Default::default(),
        total_token_usage: None,
        phase: Default::default(),
        task_context: Default::default(),
        turn_count: 0,
        token_budget: None,
        sandbox_info: None,
        termination_reason: None,
        is_seed_candidate: false,
        git_head_at_creation: Some(pre_head),
        pre_session_porcelain: Some(pre_porcelain),
        last_return_packet: None,
        change_id: None,
        spec_id: None,
        vcs_identity: None,
        identity_version: 2,
        fork_call_timestamps: Vec::new(),
    };
    let mut session_result = SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "ok".to_string(),
        tool: "codex".to_string(),
        started_at: chrono::Utc::now(),
        completed_at: chrono::Utc::now(),
        events_count: 0,
        artifacts: vec![],
        peak_memory_mb: None,
        manager_fields: Default::default(),
    };

    let buffer = SharedLogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(buffer.clone())
        .without_time()
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        maybe_record_repo_write_audit(&ctx, &session, &mut session_result);
    });

    assert_eq!(session_result.status, "success");
    assert!(
        session_result
            .manager_fields
            .artifacts
            .as_ref()
            .and_then(|value| value.get("repo_write_audit"))
            .is_some()
    );
    assert!(
        buffer
            .contents()
            .contains("repo-write audit warning artifact failed to persist; ignoring")
    );
}

#[test]
fn reused_session_audit_uses_per_execution_baseline_not_session_creation() {
    let repo = tempfile::tempdir().unwrap();
    run_git(repo.path(), &["init"]);
    run_git(repo.path(), &["config", "user.email", "test@example.com"]);
    run_git(repo.path(), &["config", "user.name", "Test User"]);
    std::fs::write(repo.path().join("tracked.txt"), "initial\n").unwrap();
    run_git(repo.path(), &["add", "tracked.txt"]);
    run_git(repo.path(), &["commit", "-m", "init"]);

    let session_creation_head = detect_git_head(repo.path()).unwrap();
    let session_creation_porcelain = git_status_porcelain(repo.path());

    std::fs::write(repo.path().join("tracked.txt"), "turn one\n").unwrap();
    run_git(repo.path(), &["commit", "-am", "turn one"]);

    let turn_two_snapshot = PreExecutionSnapshot {
        head: detect_git_head(repo.path()).unwrap(),
        porcelain: Some(git_status_porcelain(repo.path())),
    };

    let executor = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: CodexRuntimeMetadata::current(),
    };
    let global_config = GlobalConfig::default();
    let session_dir = repo.path().join("session-dir");
    std::fs::create_dir_all(&session_dir).unwrap();
    let ctx = PostExecContext {
        executor: &executor,
        prompt: "Read-only: inspect tracked.txt and summarize changes",
        effective_prompt: "Read-only: inspect tracked.txt and summarize changes",
        task_type: Some("run"),
        readonly_project_root: true,
        project_root: repo.path(),
        config: None,
        global_config: Some(&global_config),
        session_dir,
        sessions_root: "test-root".to_string(),
        execution_start_time: chrono::Utc::now(),
        hooks_config: &csa_hooks::HooksConfig::default(),
        memory_project_key: None,
        provider_session_id: None,
        events_count: 0,
        transcript_artifacts: vec![],
        changed_paths: vec![],
        pre_exec_snapshot: Some(turn_two_snapshot),
    };
    let session = MetaSessionState {
        meta_session_id: "01TESTREUSEDSESSIONAUDIT000".to_string(),
        description: None,
        project_path: repo.path().display().to_string(),
        branch: None,
        created_at: chrono::Utc::now(),
        last_accessed: chrono::Utc::now(),
        genealogy: Default::default(),
        tools: Default::default(),
        context_status: Default::default(),
        total_token_usage: None,
        phase: Default::default(),
        task_context: Default::default(),
        turn_count: 1,
        token_budget: None,
        sandbox_info: None,
        termination_reason: None,
        is_seed_candidate: false,
        git_head_at_creation: Some(session_creation_head),
        pre_session_porcelain: Some(session_creation_porcelain),
        last_return_packet: None,
        change_id: None,
        spec_id: None,
        vcs_identity: None,
        identity_version: 2,
        fork_call_timestamps: Vec::new(),
    };
    let mut session_result = SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "ok".to_string(),
        tool: "codex".to_string(),
        started_at: chrono::Utc::now(),
        completed_at: chrono::Utc::now(),
        events_count: 0,
        artifacts: vec![],
        peak_memory_mb: None,
        manager_fields: Default::default(),
    };

    maybe_record_repo_write_audit(&ctx, &session, &mut session_result);

    assert!(
        session_result
            .manager_fields
            .artifacts
            .as_ref()
            .and_then(|value| value.get("repo_write_audit"))
            .is_none()
    );
}

#[test]
fn first_execution_falls_back_to_session_creation_baseline_when_per_exec_capture_failed() {
    let repo = tempfile::tempdir().unwrap();
    run_git(repo.path(), &["init"]);
    run_git(repo.path(), &["config", "user.email", "test@example.com"]);
    run_git(repo.path(), &["config", "user.name", "Test User"]);
    std::fs::write(repo.path().join("tracked.txt"), "before\n").unwrap();
    run_git(repo.path(), &["add", "tracked.txt"]);
    run_git(repo.path(), &["commit", "-m", "init"]);

    let pre_head = detect_git_head(repo.path()).unwrap();
    let pre_porcelain = git_status_porcelain(repo.path());
    std::fs::write(repo.path().join("tracked.txt"), "after\n").unwrap();

    let executor = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: CodexRuntimeMetadata::current(),
    };
    let global_config = GlobalConfig::default();
    let session_dir = repo.path().join("session-dir");
    std::fs::create_dir_all(&session_dir).unwrap();
    let ctx = PostExecContext {
        executor: &executor,
        prompt: "Read-only: inspect tracked.txt and summarize changes",
        effective_prompt: "Read-only: inspect tracked.txt and summarize changes",
        task_type: Some("run"),
        readonly_project_root: true,
        project_root: repo.path(),
        config: None,
        global_config: Some(&global_config),
        session_dir,
        sessions_root: "test-root".to_string(),
        execution_start_time: chrono::Utc::now(),
        hooks_config: &csa_hooks::HooksConfig::default(),
        memory_project_key: None,
        provider_session_id: None,
        events_count: 0,
        transcript_artifacts: vec![],
        changed_paths: vec![],
        pre_exec_snapshot: None,
    };
    let session = MetaSessionState {
        meta_session_id: "01TESTAUDITFALLBACK000000000".to_string(),
        description: None,
        project_path: repo.path().display().to_string(),
        branch: None,
        created_at: chrono::Utc::now(),
        last_accessed: chrono::Utc::now(),
        genealogy: Default::default(),
        tools: Default::default(),
        context_status: Default::default(),
        total_token_usage: None,
        phase: Default::default(),
        task_context: Default::default(),
        turn_count: 0,
        token_budget: None,
        sandbox_info: None,
        termination_reason: None,
        is_seed_candidate: false,
        git_head_at_creation: Some(pre_head),
        pre_session_porcelain: Some(pre_porcelain),
        last_return_packet: None,
        change_id: None,
        spec_id: None,
        vcs_identity: None,
        identity_version: 2,
        fork_call_timestamps: Vec::new(),
    };
    let mut session_result = SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "ok".to_string(),
        tool: "codex".to_string(),
        started_at: chrono::Utc::now(),
        completed_at: chrono::Utc::now(),
        events_count: 0,
        artifacts: vec![],
        peak_memory_mb: None,
        manager_fields: Default::default(),
    };

    let buffer = SharedLogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(buffer.clone())
        .without_time()
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        maybe_record_repo_write_audit(&ctx, &session, &mut session_result);
    });

    assert!(
        session_result
            .manager_fields
            .artifacts
            .as_ref()
            .and_then(|value| value.get("repo_write_audit"))
            .is_some()
    );
    assert!(buffer.contents().contains(
        "repo-write audit falling back to session-creation baseline because per-execution capture is unavailable"
    ));
}

fn run_git(repo: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: stdout={} stderr={}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn detect_git_head(repo: &std::path::Path) -> anyhow::Result<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo)
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "git rev-parse HEAD failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn git_status_porcelain(repo: &std::path::Path) -> String {
    let output = std::process::Command::new("git")
        .args(["status", "--porcelain=v1", "-z"])
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
