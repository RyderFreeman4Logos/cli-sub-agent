use super::{
    PostExecGateCommandExit, PostExecGateCommandOutcome, PostExecGateOutcome,
    execute_post_exec_gate_command, maybe_run_post_exec_gate_with_runner,
};
use crate::pipeline_env::MergedEnvRequest;
use crate::pipeline_sandbox::{
    SandboxResolution, SandboxResolveInput, resolve_sandbox_options_with_overrides,
};
use crate::run_resource_overrides::RunResourceOverrides;
use csa_config::{PostExecGateConfig, ProjectConfig, ProjectMeta, ResourcesConfig, RunConfig};
use csa_process::StreamMode;
use csa_session::{SessionPhase, SessionResult, TaskContext};
use std::{collections::HashMap, path::Path, process::Command};
use tempfile::tempdir;

#[tokio::test]
async fn post_exec_gate_residual_processes_returns_structured_diagnostic() {
    let project_dir = tempdir().unwrap();
    init_clean_git_repo(project_dir.path());
    std::fs::write(project_dir.path().join("tracked.txt"), "changed\n").unwrap();
    let config = config_with_gate(PostExecGateConfig {
        command: "just pre-commit".to_string(),
        ..Default::default()
    });

    let outcome = maybe_run_post_exec_gate_with_runner(
        project_dir.path(),
        "Implement the fix in tracked.txt",
        Some("01TESTPOSTEXECGATERESIDUAL000"),
        Some(&config),
        None,
        None,
        |_command, _cwd, _timeout_seconds, _extra_env| {
            Box::pin(async {
                Ok(PostExecGateCommandOutcome {
                    exit: PostExecGateCommandExit::ResidualProcesses,
                    captured_output: String::new(),
                })
            })
        },
    )
    .await
    .expect("gate command should return a typed outcome");

    let PostExecGateOutcome::Failed(failure) = outcome else {
        panic!("expected typed gate failure, got {outcome:?}");
    };

    assert!(!failure.is_timeout());
    assert_eq!(failure.report_exit_code(), 125);
    assert!(
        failure
            .diagnostic
            .contains("csa: post-exec gate left live process-group members")
    );
    assert!(failure.diagnostic.contains("gate command: just pre-commit"));
    assert!(
        failure
            .diagnostic
            .contains("employee session: 01TESTPOSTEXECGATERESIDUAL000")
    );
}

#[tokio::test]
#[cfg(unix)]
async fn execute_post_exec_gate_command_fails_closed_without_sigkilling_residual_holder() {
    let project_dir = tempdir().unwrap();
    let sentinel = project_dir.path().join("residual-holder-survived");
    let env = HashMap::from([(
        "GATE_TEST_SENTINEL".to_string(),
        sentinel.to_string_lossy().into_owned(),
    )]);
    let command = r#"(trap 'exec 1>&- 2>&-; sleep 2; : > "$GATE_TEST_SENTINEL"; exit 0' TERM; sleep 30) & exit 0"#;

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        execute_post_exec_gate_command(command, project_dir.path(), 30, Some(env)),
    )
    .await
    .expect("gate must not hang")
    .expect("post-exec gate command should run");

    assert_eq!(outcome.exit, PostExecGateCommandExit::ResidualProcesses);
    assert!(outcome.captured_output.contains("live process-group"));
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    assert!(
        sentinel.exists(),
        "closed-pipe residual detection must fail closed without post-reap group signaling; \
         an absent sentinel means the residual holder was killed after the PGID was no longer anchored"
    );
}

#[tokio::test]
#[cfg(unix)]
async fn fix_finding_2348_harness_covers_env_identity_amend_and_residual_gate() {
    let _env_lock = crate::test_env_lock::TEST_ENV_LOCK.lock().await;
    let td = tempdir().unwrap();
    let project_root = td.path().join("repo");
    let state_home = td.path().join("xdg-state");
    let cargo_home = td.path().join("cargo-home");
    let cargo_target_dir = td.path().join("target-explicit");
    let cargo_install_root = cargo_target_dir.join("cargo-install-root");
    for dir in [
        &project_root,
        &state_home,
        &cargo_home,
        &cargo_target_dir,
        &cargo_install_root,
    ] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let _home_guard = crate::test_env_lock::ScopedEnvVarRestore::set("HOME", td.path());
    let _state_guard =
        crate::test_env_lock::ScopedEnvVarRestore::set("XDG_STATE_HOME", &state_home);
    init_clean_git_repo(&project_root);

    let original_review = csa_session::create_session_fresh(
        &project_root,
        Some("original #2348 review"),
        None,
        Some("codex"),
    )
    .unwrap();
    let original_review_id = original_review.meta_session_id.clone();
    csa_session::save_result(
        &project_root,
        &original_review_id,
        &make_result("failure", 1),
    )
    .unwrap();

    let mut fix_session = csa_session::create_session_fresh(
        &project_root,
        Some("fix-finding #2348"),
        Some(&original_review_id),
        Some("codex"),
    )
    .unwrap();
    fix_session.phase = SessionPhase::Active;
    fix_session.task_context = TaskContext {
        task_type: Some("review_fix_finding".to_string()),
        tier_name: None,
    };
    csa_session::save_session(&fix_session).unwrap();
    let fix_session_id = fix_session.meta_session_id.clone();
    let wrapper =
        csa_session::create_session_fresh(&project_root, Some("fix-finding wrapper"), None, None)
            .unwrap();
    let wrapper_id = wrapper.meta_session_id;
    csa_session::write_resume_target(&project_root, &wrapper_id, &fix_session_id).unwrap();

    let config = sandbox_config();
    let explicit_env = HashMap::from([
        ("HOME".to_string(), td.path().to_string_lossy().into_owned()),
        (
            "PATH".to_string(),
            std::env::var("PATH").unwrap_or_default(),
        ),
        (
            csa_core::env::CARGO_HOME_ENV_KEY.to_string(),
            cargo_home.to_string_lossy().into_owned(),
        ),
        (
            csa_core::env::CARGO_TARGET_DIR_ENV_KEY.to_string(),
            cargo_target_dir.to_string_lossy().into_owned(),
        ),
        (
            csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY.to_string(),
            cargo_install_root.to_string_lossy().into_owned(),
        ),
    ]);
    let merged_env = crate::pipeline_env::build_merged_env(MergedEnvRequest {
        extra_env: Some(&explicit_env),
        config: Some(&config),
        global_config: None,
        project_root: Some(&project_root),
        tool_name: "codex",
        current_depth: 0,
        pattern_internal: false,
        allow_git_push: false,
    });
    assert_eq!(
        merged_env.get(csa_core::env::CARGO_HOME_ENV_KEY),
        explicit_env.get(csa_core::env::CARGO_HOME_ENV_KEY),
        "CARGO_HOME"
    );
    assert_eq!(
        merged_env
            .get(csa_core::env::CARGO_TARGET_DIR_ENV_KEY)
            .map(String::as_str),
        Some(cargo_target_dir.to_str().expect("target utf8")),
        "CARGO_TARGET_DIR"
    );
    assert_eq!(
        merged_env
            .get(csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY)
            .map(String::as_str),
        Some(cargo_install_root.to_str().expect("install root utf8")),
        "CARGO_INSTALL_ROOT"
    );

    let sandbox = match resolve_sandbox_options_with_overrides(
        SandboxResolveInput {
            config: Some(&config),
            tool_name: "codex",
            session_id: &fix_session_id,
            project_root: &project_root,
            stream_mode: StreamMode::BufferOnly,
            idle_timeout_seconds: 120,
            liveness_dead_seconds: 600,
            initial_response_timeout_seconds: Some(120),
            no_fs_sandbox: false,
            allow_user_daemon_ipc: false,
            readonly_project_root: false,
            extra_writable: &[],
            extra_readable: &[],
            execution_env: Some(&merged_env),
        },
        RunResourceOverrides::default(),
    ) {
        SandboxResolution::Ok(opts) => opts.sandbox.expect("expected sandbox context"),
        SandboxResolution::RequiredButUnavailable(message) => {
            panic!("sandbox should be available for #2348 harness: {message}")
        }
    };
    for path in [&cargo_home, &cargo_target_dir, &cargo_install_root] {
        assert!(
            sandbox
                .isolation_plan
                .writable_paths
                .contains(&path.canonicalize().unwrap()),
            "{} must be in writable sandbox contract: {:?}",
            path.display(),
            sandbox.isolation_plan.writable_paths
        );
    }

    let before_head = git_stdout(&project_root, &["rev-parse", "HEAD"]);
    std::fs::write(project_root.join("tracked.txt"), "fixed\n").unwrap();
    run_git(&project_root, &["add", "tracked.txt"]);
    run_git(
        &project_root,
        &["commit", "--amend", "-q", "-m", "initial fixed"],
    );
    let after_head = git_stdout(&project_root, &["rev-parse", "HEAD"]);
    assert_ne!(
        before_head.trim(),
        after_head.trim(),
        "fix-finding amend should advance HEAD in the synthetic repo"
    );
    assert!(
        git_stdout(&project_root, &["status", "--short"])
            .trim()
            .is_empty(),
        "synthetic fix-finding repo should be clean after amend"
    );

    let fix_result = make_result("success", 0);
    csa_session::save_result(&project_root, &fix_session_id, &fix_result).unwrap();
    let fix_dir = csa_session::get_session_dir(&project_root, &fix_session_id).unwrap();
    let summary =
        crate::session_cmds_daemon::render_wait_result_summary(&fix_dir, &wrapper_id, &fix_result);
    assert!(
        summary.contains(&format!("Session: {wrapper_id}")),
        "{summary}"
    );
    assert!(
        summary.contains(&format!("Target session: {fix_session_id}")),
        "{summary}"
    );
    assert!(
        !summary.contains(&format!("Session: {original_review_id}")),
        "{summary}"
    );

    let sentinel = td.path().join("residual-holder-survived");
    let residual_env = HashMap::from([(
        "GATE_TEST_SENTINEL".to_string(),
        sentinel.to_string_lossy().into_owned(),
    )]);
    let command = r#"(trap 'exec 1>&- 2>&-; sleep 2; : > "$GATE_TEST_SENTINEL"; exit 0' TERM; sleep 30) & exit 0"#;
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        execute_post_exec_gate_command(command, &project_root, 30, Some(residual_env)),
    )
    .await
    .expect("gate must not hang")
    .expect("post-exec gate command should run");
    assert_eq!(outcome.exit, PostExecGateCommandExit::ResidualProcesses);
    assert!(outcome.captured_output.contains("live process-group"));
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    assert!(
        sentinel.exists(),
        "residual gate detection must fail closed without killing a post-reap group by stale PGID"
    );
}

fn sandbox_config() -> ProjectConfig {
    toml::from_str(
        r#"
[resources]
memory_max_mb = 2048
enforcement_mode = "best-effort"
"#,
    )
    .expect("sandbox config should parse")
}

fn config_with_gate(gate: PostExecGateConfig) -> ProjectConfig {
    ProjectConfig {
        schema_version: csa_config::config::CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: chrono::Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        github: None,
        session: Default::default(),
        memory: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        hooks: Default::default(),
        run: RunConfig {
            allow_base_branch_working: false,
            writer_must_commit: false,
            large_diff_warning: Default::default(),
            post_exec_gate: gate,
        },
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    }
}

fn make_result(status: &str, exit_code: i32) -> SessionResult {
    let now = chrono::Utc::now();
    SessionResult {
        post_exec_gate: None,
        status: status.to_string(),
        exit_code,
        summary: "summary".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: Vec::new(),
        ..Default::default()
    }
}

fn run_git(project_root: &Path, args: &[&str]) -> std::process::Output {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {} failed\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn git_stdout(project_root: &Path, args: &[&str]) -> String {
    String::from_utf8(run_git(project_root, args).stdout).expect("git stdout utf8")
}

fn init_clean_git_repo(project_root: &std::path::Path) {
    run_git(project_root, &["init", "--initial-branch", "main"]);
    run_git(project_root, &["config", "user.email", "test@example.com"]);
    run_git(project_root, &["config", "user.name", "Test"]);
    run_git(project_root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(project_root.join("tracked.txt"), "initial\n").unwrap();
    run_git(project_root, &["add", "tracked.txt"]);
    run_git(project_root, &["commit", "-m", "initial"]);
}
