use super::*;
use crate::plan_cmd::PlanRunArgs;
use crate::test_env_lock::isolate_user_config_locked as iso;
use std::process::Command;

fn make_args() -> PlanRunArgs {
    PlanRunArgs {
        file: Some("workflow.toml".to_string()),
        pattern: None,
        vars: vec![],
        tool_override: None,
        model_spec_override: None,
        dry_run: false,
        chunked: false,
        resume: None,
        complete_manual_step: None,
        cd: None,
        no_fs_sandbox: false,
        resources: RunResourceOverrides::absent(),
        current_depth: 0,
        pipeline_source: crate::plan_cmd::PlanRunPipelineSource::DirectPlanRun,
        startup_env: crate::startup_env::StartupSubtreeEnv::default(),
    }
}

fn run_git(project_root: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .output()
        .expect("git command should start");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_plan_test_repo(project_root: &std::path::Path) {
    run_git(project_root, &["init", "-b", "main"]);
    run_git(
        project_root,
        &["config", "user.email", "csa-test@example.com"],
    );
    run_git(project_root, &["config", "user.name", "CSA Test"]);
    run_git(project_root, &["config", "core.excludesFile", "/dev/null"]);
    std::fs::write(
        project_root.join(".git").join("info").join("exclude"),
        ".csa/\n",
    )
    .expect("write repo-local exclude");
    std::fs::write(project_root.join("README.md"), "test repo\n").expect("write readme");
    std::fs::write(project_root.join("weave.lock"), "lock = 1\n").expect("write weave.lock");
}

fn commit_all(project_root: &std::path::Path, message: &str) {
    run_git(
        project_root,
        &["add", "README.md", "weave.lock", "workflow.toml"],
    );
    run_git(project_root, &["commit", "-m", message]);
}

fn plan_daemon_args(project_root: &std::path::Path) -> PlanRunArgs {
    let mut args = make_args();
    args.file = Some("workflow.toml".to_string());
    args.cd = Some(project_root.display().to_string());
    args
}

fn prepare_plan_session(
    project_root: &std::path::Path,
    description: &str,
) -> (String, std::path::PathBuf) {
    let session_id = csa_session::new_session_id();
    let session_dir = csa_session::get_session_dir(project_root, &session_id)
        .expect("session dir should resolve");
    persist_placeholder_plan_session(project_root, &session_dir, &session_id, description)
        .expect("placeholder plan session should persist");
    (session_id, session_dir)
}

#[test]
fn describe_uses_pattern_name_when_set() {
    let mut args = make_args();
    args.file = None;
    args.pattern = Some("dev2merge".to_string());
    assert_eq!(describe_plan_run(&args), "plan: dev2merge");
}

#[test]
fn describe_falls_back_to_file_path() {
    let args = make_args();
    assert_eq!(describe_plan_run(&args), "plan: workflow.toml");
}

#[test]
fn describe_handles_resume_form() {
    let mut args = make_args();
    args.file = None;
    args.resume = Some("/tmp/journal.json".to_string());
    assert_eq!(describe_plan_run(&args), "plan: --resume /tmp/journal.json");
}

#[test]
fn describe_unknown_when_no_source_provided() {
    let mut args = make_args();
    args.file = None;
    assert_eq!(describe_plan_run(&args), "plan: (unknown workflow)");
}

#[tokio::test]
async fn daemon_child_failed_plan_writes_structured_failure_output() {
    let temp = tempfile::tempdir().expect("tempdir");
    let _user_config_env = iso(temp.path()).await;
    let project_root = temp.path().join("repo");
    std::fs::create_dir_all(&project_root).expect("repo dir should be created");
    init_plan_test_repo(&project_root);
    std::fs::write(
        project_root.join("workflow.toml"),
        r#"[workflow]
name = "failing-plan"

[[workflow.steps]]
id = 1
title = "Failing Bash"
tool = "bash"
prompt = '''
```bash
printf 'structured boom\n' >&2
exit 7
```
'''
on_fail = "abort"
"#,
    )
    .expect("write workflow");
    commit_all(&project_root, "initial");
    let (session_id, session_dir) = prepare_plan_session(&project_root, "plan: failing-plan");

    let result = handle_plan_run_daemon_child(plan_daemon_args(&project_root), &session_id).await;

    assert!(result.is_err(), "failing plan should return an error");
    let summary = csa_session::read_section(&session_dir, "summary")
        .expect("summary should load")
        .expect("summary section should exist");
    assert!(
        summary.contains("Failed step: 1 (Failing Bash) exited 7"),
        "summary must identify failed step and exit code: {summary}"
    );
    let details = csa_session::read_section(&session_dir, "details")
        .expect("details should load")
        .expect("details section should exist");
    assert!(
        details.contains("Step 1: Failing Bash")
            && details.contains("exit 7")
            && details.contains("structured boom"),
        "details must include failed step id, command, exit code, and stderr excerpt: {details}"
    );
    let persisted = csa_session::load_result(&project_root, &session_id)
        .expect("result should load")
        .expect("result.toml should exist");
    assert_eq!(persisted.exit_code, 1);
    assert!(
        persisted
            .artifacts
            .iter()
            .any(|artifact| artifact.path == "output/details.md"),
        "result artifacts must point callers at structured details"
    );
}

#[tokio::test]
async fn daemon_child_failed_pr_bot_preserves_weave_lock_after_snapshot() {
    let temp = tempfile::tempdir().expect("tempdir");
    let _user_config_env = iso(temp.path()).await;
    let project_root = temp.path().join("repo");
    std::fs::create_dir_all(&project_root).expect("repo dir should be created");
    init_plan_test_repo(&project_root);
    std::fs::write(
        project_root.join("workflow.toml"),
        r#"[workflow]
name = "pr-bot"

[[workflow.steps]]
id = 1
title = "Dirty Main Failure"
tool = "bash"
prompt = '''
```bash
git switch main
printf 'plan drift\n' >> weave.lock
printf 'failed on main\n' >&2
exit 9
```
'''
on_fail = "abort"
"#,
    )
    .expect("write workflow");
    commit_all(&project_root, "initial");
    run_git(&project_root, &["switch", "-c", "fix/pr-bot-recovery"]);
    let (session_id, session_dir) = prepare_plan_session(&project_root, "plan: pr-bot");

    let result = handle_plan_run_daemon_child(plan_daemon_args(&project_root), &session_id).await;

    assert!(
        result.is_err(),
        "failing pr-bot plan should return an error"
    );
    let branch = Command::new("git")
        .arg("-C")
        .arg(&project_root)
        .args(["branch", "--show-current"])
        .output()
        .expect("git branch should run");
    assert_eq!(
        String::from_utf8_lossy(&branch.stdout).trim(),
        "fix/pr-bot-recovery",
        "failed pr-bot plan should restore the caller checkout"
    );
    let status = Command::new("git")
        .arg("-C")
        .arg(&project_root)
        .args(["status", "--short"])
        .output()
        .expect("git status should run");
    let status = String::from_utf8_lossy(&status.stdout);
    assert!(
        status.contains("weave.lock"),
        "failed pr-bot plan should preserve post-snapshot weave.lock changes: {status}"
    );
    let weave_lock = std::fs::read_to_string(project_root.join("weave.lock"))
        .expect("weave.lock should remain readable");
    assert!(
        weave_lock.contains("plan drift"),
        "failed pr-bot plan must not discard weave.lock content: {weave_lock}"
    );
    let details = csa_session::read_section(&session_dir, "details")
        .expect("details should load")
        .expect("details section should exist");
    assert!(
        details.contains("Recovery status: `manual-required`")
            && details.contains("Preserved dirty weave.lock")
            && details.contains("Restored checkout to fix/pr-bot-recovery"),
        "details must include structured recovery result: {details}"
    );
}

#[test]
fn daemon_child_injects_preassigned_session_into_startup_env() {
    let _env_lock = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let session_id = "01PARENTSESSION000000000000";
    let mut args = make_args();

    inject_plan_daemon_session_into_startup_env(&mut args, session_id, temp.path())
        .expect("startup env should accept daemon session context");

    let expected_session_dir =
        csa_session::get_session_dir(temp.path(), session_id).expect("session dir should resolve");
    let expected_session_dir = expected_session_dir.to_string_lossy().to_string();
    assert_eq!(args.startup_env.session_id(), Some(session_id));
    assert_eq!(
        args.startup_env.session_dir(),
        Some(expected_session_dir.as_str())
    );
}

#[test]
fn establish_foreground_plan_session_mints_session_when_absent() {
    // #1851: a top-level `--foreground` / `--resume` plan run carries no session
    // identity in its startup snapshot. Establishing the foreground session MUST
    // mint one so spawn_bash exports CSA_SESSION_DIR / CSA_SESSION_ID to every
    // workflow bash step (the mktd Save step reads `${CSA_SESSION_DIR:?...}`).
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let state_home = temp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).expect("state home should be created");
    let _state_guard = crate::test_env_lock::ScopedTestEnvVar::set("XDG_STATE_HOME", &state_home);

    let startup_env = crate::startup_env::StartupSubtreeEnv::default();
    let established =
        establish_foreground_plan_session(&startup_env, temp.path(), "plan: workflow.toml")
            .expect("foreground session should establish");

    let minted = established
        .minted_session_id
        .as_deref()
        .expect("a fresh foreground run must mint a session");
    assert_eq!(established.startup_env.session_id(), Some(minted));
    assert!(
        established.startup_env.session_dir().is_some(),
        "minted session must carry a session dir"
    );

    // Assert via the exact channel spawn_bash uses to build the bash-step env
    // (`apply_startup_child_contract_env` -> `to_csa_child_contract_env_vars`).
    let contract: std::collections::HashMap<String, String> = established
        .startup_env
        .to_csa_child_contract_env_vars()
        .into_iter()
        .collect();
    assert!(
        contract.contains_key(csa_core::env::CSA_SESSION_DIR_ENV_KEY),
        "workflow bash steps must receive CSA_SESSION_DIR"
    );
    assert!(
        contract.contains_key(csa_core::env::CSA_SESSION_ID_ENV_KEY),
        "workflow bash steps must receive CSA_SESSION_ID"
    );
}

#[test]
fn establish_foreground_plan_session_derives_dir_from_inherited_id() {
    // A nested foreground invocation whose parent exported CSA_SESSION_ID but
    // (defensively) not CSA_SESSION_DIR: derive the canonical dir from the id
    // without minting a new session.
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let session_id = "01INHERITEDID00000000000000";
    let startup_env =
        crate::startup_env::StartupSubtreeEnv::from_values(std::collections::HashMap::from([(
            csa_core::env::CSA_SESSION_ID_ENV_KEY,
            session_id.to_string(),
        )]));

    let established =
        establish_foreground_plan_session(&startup_env, temp.path(), "plan: workflow.toml")
            .expect("foreground session should establish");

    assert!(established.minted_session_id.is_none());
    assert_eq!(established.startup_env.session_id(), Some(session_id));
    let expected_dir = csa_session::get_session_dir(temp.path(), session_id)
        .expect("session dir should resolve")
        .to_string_lossy()
        .to_string();
    assert_eq!(
        established.startup_env.session_dir(),
        Some(expected_dir.as_str())
    );
}

#[test]
fn establish_foreground_plan_session_preserves_inherited_identity() {
    // A nested foreground invocation whose parent exported the full contract
    // (id + dir): reuse it untouched, mint nothing.
    let startup_env =
        crate::startup_env::StartupSubtreeEnv::from_values(std::collections::HashMap::from([
            (
                csa_core::env::CSA_SESSION_ID_ENV_KEY,
                "01PARENTID0000000000000000".to_string(),
            ),
            (
                csa_core::env::CSA_SESSION_DIR_ENV_KEY,
                "/repo/parent-session".to_string(),
            ),
        ]));

    let established = establish_foreground_plan_session(
        &startup_env,
        std::path::Path::new("/repo"),
        "plan: workflow.toml",
    )
    .expect("foreground session should establish");

    assert!(established.minted_session_id.is_none());
    assert_eq!(
        established.startup_env.session_id(),
        Some("01PARENTID0000000000000000")
    );
    assert_eq!(
        established.startup_env.session_dir(),
        Some("/repo/parent-session")
    );
}

#[test]
fn forwarded_args_strip_through_plan_run() {
    let argv = vec![
        "csa".to_string(),
        "plan".to_string(),
        "run".to_string(),
        "patterns/dev2merge/workflow.toml".to_string(),
        "--sa-mode".to_string(),
        "true".to_string(),
        "--var".to_string(),
        "FEATURE_INPUT=test".to_string(),
    ];
    let forwarded = build_forwarded_plan_args(&argv);
    assert_eq!(
        forwarded,
        vec![
            "patterns/dev2merge/workflow.toml",
            "--sa-mode",
            "true",
            "--var",
            "FEATURE_INPUT=test",
        ]
    );
}

#[test]
fn forwarded_args_drop_foreground_flag() {
    let argv = vec![
        "csa".to_string(),
        "plan".to_string(),
        "run".to_string(),
        "--foreground".to_string(),
        "workflow.toml".to_string(),
    ];
    // The `--foreground` flag is the parent-only opt-out and must not be
    // forwarded to the daemon child (which IS the worker, not a re-spawn).
    let forwarded = build_forwarded_plan_args(&argv);
    assert_eq!(forwarded, vec!["workflow.toml"]);
}

#[test]
fn forwarded_args_preserve_no_fs_sandbox_flag() {
    let argv = vec![
        "csa".to_string(),
        "plan".to_string(),
        "run".to_string(),
        "--no-fs-sandbox".to_string(),
        "workflow.toml".to_string(),
    ];
    let forwarded = build_forwarded_plan_args(&argv);
    assert_eq!(forwarded, vec!["--no-fs-sandbox", "workflow.toml"]);
}

#[test]
fn forwarded_args_preserve_plan_memory_overrides() {
    let argv = vec![
        "csa".to_string(),
        "plan".to_string(),
        "run".to_string(),
        "--memory-max-mb".to_string(),
        "9103".to_string(),
        "--min-free-memory-mb".to_string(),
        "193".to_string(),
        "workflow.toml".to_string(),
    ];

    let forwarded = build_forwarded_plan_args(&argv);

    assert_eq!(
        forwarded,
        vec![
            "--memory-max-mb",
            "9103",
            "--min-free-memory-mb",
            "193",
            "workflow.toml",
        ]
    );
}

#[test]
fn forwarded_args_handle_global_flags_before_plan() {
    let argv = vec![
        "csa".to_string(),
        "--format".to_string(),
        "json".to_string(),
        "plan".to_string(),
        "run".to_string(),
        "--pattern".to_string(),
        "dev2merge".to_string(),
    ];
    let forwarded = build_forwarded_plan_args(&argv);
    assert_eq!(forwarded, vec!["--pattern", "dev2merge"]);
}

#[test]
fn forwarded_args_empty_when_plan_missing() {
    let argv = vec!["csa".to_string(), "run".to_string()];
    assert!(build_forwarded_plan_args(&argv).is_empty());
}

#[test]
fn forwarded_args_preserve_foreground_after_double_dash() {
    // F3: stripping is `--`-aware. A literal `--foreground` AFTER a `--`
    // positional separator is a workflow argument, not the parent-only opt-out
    // flag, and must be forwarded intact.
    let argv = vec![
        "csa".to_string(),
        "plan".to_string(),
        "run".to_string(),
        "--foreground".to_string(),
        "workflow.toml".to_string(),
        "--".to_string(),
        "--foreground".to_string(),
    ];
    let forwarded = build_forwarded_plan_args(&argv);
    assert_eq!(
        forwarded,
        vec!["workflow.toml", "--", "--foreground"],
        "literal --foreground after `--` must be preserved as a positional"
    );
}

// ---------------------------------------------------------------------------
// F1 — depth-aware daemon flip gating
// ---------------------------------------------------------------------------

fn base_input() -> ForegroundDecisionInput {
    ForegroundDecisionInput {
        foreground: false,
        dry_run: false,
        chunked: false,
        has_resume: false,
        current_depth: 0,
        nested_env: false,
    }
}

#[test]
fn dispatch_with_no_session_env_and_depth_zero_takes_daemon_path() {
    // Top-level user invocation: clean env, depth=0, no opt-out flags →
    // daemonize (needs_foreground=false).
    assert!(!decide_needs_foreground(base_input()));
}

#[test]
fn dispatch_with_csa_depth_gt_zero_forces_foreground() {
    // Nested via bash-step CSA_DEPTH bump (plan_cmd_exec::spawn_bash sets
    // CSA_DEPTH = parent_depth + 1 for every workflow bash step).
    let mut input = base_input();
    input.current_depth = 1;
    assert!(decide_needs_foreground(input));
}

#[test]
fn dispatch_with_csa_session_id_in_env_forces_foreground() {
    // Nested via inherited session-marker env (handle_plan_run_daemon_child
    // sets CSA_SESSION_ID; bash steps inherit it; the nested csa plan run
    // invocation reads it back).
    let mut input = base_input();
    input.nested_env = true;
    assert!(decide_needs_foreground(input));
}

#[test]
fn dispatch_explicit_foreground_still_runs_inline_at_depth_zero() {
    // User opt-out at root depth — the existing --foreground escape hatch
    // must keep working independently of nested-invocation detection.
    let mut input = base_input();
    input.foreground = true;
    assert!(decide_needs_foreground(input));
}

#[test]
fn dispatch_dry_run_forces_foreground_at_depth_zero() {
    // --dry-run prints the plan synchronously; daemonizing would lose the
    // stdout output the user is asking for.
    let mut input = base_input();
    input.dry_run = true;
    assert!(decide_needs_foreground(input));
}

#[test]
fn dispatch_chunked_forces_foreground_at_depth_zero() {
    let mut input = base_input();
    input.chunked = true;
    assert!(decide_needs_foreground(input));
}

#[test]
fn dispatch_resume_forces_foreground_at_depth_zero() {
    let mut input = base_input();
    input.has_resume = true;
    assert!(decide_needs_foreground(input));
}

#[test]
fn dispatch_depth_gt_zero_overrides_all_opt_outs() {
    // Even if a future caller tries to "force daemon" by passing a clean
    // input shape, depth>0 is sticky — nested daemonization is never safe.
    let input = ForegroundDecisionInput {
        foreground: false,
        dry_run: false,
        chunked: false,
        has_resume: false,
        current_depth: 3,
        nested_env: false,
    };
    assert!(decide_needs_foreground(input));
}

// ---------------------------------------------------------------------------
// F1 — env-marker probe
// ---------------------------------------------------------------------------
//
// The env-mutating tests below use serial_test to avoid races with other
// CSA_*_SESSION_ID readers in the test binary. Each test snapshots the
// markers it touches and restores them on exit.

mod env_probe {
    use super::*;
    use serial_test::serial;

    const MARKERS: &[&str] = &[
        "CSA_SESSION_ID",
        "CSA_DAEMON_SESSION_ID",
        "CSA_PARENT_SESSION_ID",
    ];

    fn snapshot_markers() -> Vec<(&'static str, Option<String>)> {
        MARKERS
            .iter()
            .map(|k| (*k, std::env::var(k).ok()))
            .collect()
    }

    /// Restore previously-snapshotted env markers. Called at the end of
    /// every test in this module via the [`EnvGuard`] RAII wrapper.
    fn restore_markers(snapshot: Vec<(&'static str, Option<String>)>) {
        for (key, value) in snapshot {
            // SAFETY: test-only; runs after all reads and before module
            // teardown. serial_test ensures no concurrent access.
            unsafe {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    struct EnvGuard {
        snapshot: Option<Vec<(&'static str, Option<String>)>>,
    }
    impl EnvGuard {
        fn capture_and_clear() -> Self {
            let snapshot = snapshot_markers();
            for key in MARKERS {
                // SAFETY: test-only; serial_test ensures no concurrent access.
                unsafe { std::env::remove_var(key) };
            }
            Self {
                snapshot: Some(snapshot),
            }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(snap) = self.snapshot.take() {
                restore_markers(snap);
            }
        }
    }

    fn set_marker(key: &str, value: &str) {
        // SAFETY: test-only; serial_test ensures no concurrent access.
        unsafe { std::env::set_var(key, value) };
    }

    #[test]
    #[serial]
    fn nested_env_clean_returns_false() {
        let _guard = EnvGuard::capture_and_clear();
        assert!(!nested_session_env_present(
            &crate::startup_env::StartupSubtreeEnv::default()
        ));
    }

    #[test]
    #[serial]
    fn nested_env_with_startup_csa_session_id_returns_true() {
        let _guard = EnvGuard::capture_and_clear();
        let startup_env =
            crate::startup_env::StartupSubtreeEnv::from_values(std::collections::HashMap::from([
                (
                    csa_core::env::CSA_SESSION_ID_ENV_KEY,
                    "01TESTFAKE000000000000000".to_string(),
                ),
            ]));
        assert!(nested_session_env_present(&startup_env));
    }

    #[test]
    #[serial]
    fn nested_env_ignores_live_csa_session_id_after_startup_scrub() {
        let _guard = EnvGuard::capture_and_clear();
        set_marker("CSA_SESSION_ID", "01TESTFAKE000000000000000");
        assert!(!nested_session_env_present(
            &crate::startup_env::StartupSubtreeEnv::default()
        ));
    }

    #[test]
    #[serial]
    fn nested_env_with_csa_daemon_session_id_returns_true() {
        let _guard = EnvGuard::capture_and_clear();
        set_marker("CSA_DAEMON_SESSION_ID", "01TESTFAKE000000000000000");
        assert!(nested_session_env_present(
            &crate::startup_env::StartupSubtreeEnv::default()
        ));
    }

    #[test]
    #[serial]
    fn nested_env_with_csa_parent_session_id_returns_true() {
        let _guard = EnvGuard::capture_and_clear();
        set_marker("CSA_PARENT_SESSION_ID", "01TESTFAKE000000000000000");
        assert!(nested_session_env_present(
            &crate::startup_env::StartupSubtreeEnv::default()
        ));
    }

    #[test]
    #[serial]
    fn nested_env_with_empty_marker_returns_false() {
        // Edge case: empty string is treated as "not set" so callers that
        // accidentally exported an empty value don't trigger the gate.
        let _guard = EnvGuard::capture_and_clear();
        set_marker("CSA_DAEMON_SESSION_ID", "");
        assert!(!nested_session_env_present(
            &crate::startup_env::StartupSubtreeEnv::default()
        ));
    }
}
