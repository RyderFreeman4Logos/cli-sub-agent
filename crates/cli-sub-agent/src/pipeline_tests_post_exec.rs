use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_session::{create_session, get_session_dir, load_session};
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::Command;

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
        peak_memory_mb: None,
        manager_fields: Default::default(),
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
}

fn run_command(repo: &Path, program: &str, args: &[&str]) {
    let output = Command::new(program)
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap_or_else(|err| panic!("spawn {program}: {err}"));
    assert!(
        output.status.success(),
        "{program} {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn setup_colocated_jj_git_repo(repo: &Path) {
    std::fs::create_dir_all(repo).expect("create repo dir");
    run_command(repo, "git", &["init"]);
    run_command(
        repo,
        "git",
        &["config", "user.email", "csa-test@example.com"],
    );
    run_command(repo, "git", &["config", "user.name", "CSA Test"]);
    run_command(repo, "jj", &["git", "init", "--colocate"]);
    run_command(
        repo,
        "jj",
        &[
            "config",
            "set",
            "--repo",
            "user.email",
            "csa-test@example.com",
        ],
    );
    run_command(
        repo,
        "jj",
        &["config", "set", "--repo", "user.name", "CSA Test"],
    );
}

fn jj_log_descriptions(repo: &Path) -> String {
    let output = Command::new("jj")
        .args(["log", "--no-graph", "-T", "description ++ \"\\n\""])
        .current_dir(repo)
        .output()
        .expect("run jj log");
    assert!(
        output.status.success(),
        "jj log failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn post_exec_jj_config(auto_snapshot: bool) -> csa_config::ProjectConfig {
    toml::from_str(&format!(
        r#"
schema_version = 1

[vcs]
auto_snapshot = {auto_snapshot}
snapshot_trigger = "post-run"
"#
    ))
    .expect("parse project config")
}

#[tokio::test]
async fn process_execution_result_respects_vcs_auto_snapshot_gate_for_colocated_jj_repo() {
    if which::which("jj").is_err() {
        eprintln!("skipping real jj post-exec snapshot test because jj is not installed");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let mut sandbox = ScopedSessionSandbox::new(&tmp).await;
    sandbox.track_env("XDG_CONFIG_HOME");
    let config_home = tmp.path().join("config-home");
    std::fs::create_dir_all(&config_home).expect("create config home");
    // SAFETY: ScopedSessionSandbox holds TEST_ENV_LOCK for this test.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", &config_home) };

    let project_root = tmp.path().join("repo");
    setup_colocated_jj_git_repo(&project_root);
    std::fs::write(project_root.join("tracked.txt"), "first\n").expect("write tracked file");
    let changed_paths = vec!["tracked.txt".to_string()];
    let executor = csa_executor::Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: csa_executor::ClaudeCodeRuntimeMetadata::current(),
    };
    let hooks_config = csa_hooks::HooksConfig::default();

    let disabled_config = post_exec_jj_config(false);
    let mut disabled_session =
        create_session(&project_root, Some("disabled"), None, Some("claude-code"))
            .expect("create disabled session");
    let disabled_session_dir = get_session_dir(&project_root, &disabled_session.meta_session_id)
        .expect("disabled session dir");
    let disabled_ctx = PostExecContext {
        executor: &executor,
        prompt: "test prompt",
        effective_prompt: "test prompt",
        task_type: Some("run"),
        readonly_project_root: false,
        project_root: &project_root,
        config: Some(&disabled_config),
        global_config: None,
        session_dir: disabled_session_dir,
        sessions_root: "test-root".to_string(),
        execution_start_time: chrono::Utc::now() - chrono::Duration::seconds(2),
        hooks_config: &hooks_config,
        memory_project_key: None,
        provider_session_id: None,
        events_count: 1,
        transcript_artifacts: vec![],
        changed_paths: changed_paths.clone(),
        pre_exec_snapshot: None,
        has_tool_calls: true,
        sa_mode: false,
    };
    let mut disabled_result = csa_process::ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "ok".to_string(),
        exit_code: 0,
        peak_memory_mb: None,
    };

    process_execution_result(disabled_ctx, &mut disabled_session, &mut disabled_result)
        .await
        .expect("process disabled post-exec");
    assert!(!jj_log_descriptions(&project_root).contains("disabled"));

    let enabled_config = post_exec_jj_config(true);
    let mut enabled_session =
        create_session(&project_root, Some("enabled"), None, Some("claude-code"))
            .expect("create enabled session");
    let enabled_session_dir = get_session_dir(&project_root, &enabled_session.meta_session_id)
        .expect("enabled session dir");
    let enabled_session_id = enabled_session.meta_session_id.clone();
    let enabled_ctx = PostExecContext {
        executor: &executor,
        prompt: "test prompt",
        effective_prompt: "test prompt",
        task_type: Some("run"),
        readonly_project_root: false,
        project_root: &project_root,
        config: Some(&enabled_config),
        global_config: None,
        session_dir: enabled_session_dir,
        sessions_root: "test-root".to_string(),
        execution_start_time: chrono::Utc::now() - chrono::Duration::seconds(2),
        hooks_config: &hooks_config,
        memory_project_key: None,
        provider_session_id: None,
        events_count: 1,
        transcript_artifacts: vec![],
        changed_paths,
        pre_exec_snapshot: None,
        has_tool_calls: true,
        sa_mode: false,
    };
    let mut enabled_result = csa_process::ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "ok".to_string(),
        exit_code: 0,
        peak_memory_mb: None,
    };

    process_execution_result(enabled_ctx, &mut enabled_session, &mut enabled_result)
        .await
        .expect("process enabled post-exec");
    assert!(
        enabled_result.stderr_output.is_empty(),
        "jj snapshot aggregation should not emit warnings: {}",
        enabled_result.stderr_output
    );
    assert!(
        jj_log_descriptions(&project_root).contains(&format!("csa: {enabled_session_id}")),
        "jj log should include enabled CSA aggregate commit"
    );
    let journal_state = std::fs::read_to_string(
        get_session_dir(&project_root, &enabled_session_id)
            .expect("enabled session dir")
            .join("jj-journal-state.json"),
    )
    .expect("read jj journal state");
    assert!(
        !journal_state.contains("snapshot_revisions"),
        "successful aggregation should clear snapshot revisions: {journal_state}"
    );
}

// Handoff artifact tests are in pipeline_handoff.rs

#[test]
fn codex_exec_initial_stall_summary_forces_failure_status_in_result_toml() {
    let now = chrono::Utc::now();
    let mut result = SessionResult {
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
        peak_memory_mb: None,
        manager_fields: Default::default(),
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
        has_tool_calls: true,
        sa_mode: false,
    };
    let mut result = csa_process::ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "captured via session complete".to_string(),
        exit_code: 0,
        peak_memory_mb: None,
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
