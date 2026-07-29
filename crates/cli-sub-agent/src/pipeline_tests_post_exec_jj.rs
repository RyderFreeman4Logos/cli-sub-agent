use super::*;

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
        timeout_diagnostics: None,
        has_tool_calls: true,
        turn_count: 0,
        output_tokens: None,
        sa_mode: false,
        original_exit_code: None,
    };
    let mut disabled_result = csa_process::ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "ok".to_string(),
        exit_code: 0,
        peak_memory_mb: None,
        ..Default::default()
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
        timeout_diagnostics: None,
        has_tool_calls: true,
        turn_count: 0,
        output_tokens: None,
        sa_mode: false,
        original_exit_code: None,
    };
    let mut enabled_result = csa_process::ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "ok".to_string(),
        exit_code: 0,
        peak_memory_mb: None,
        ..Default::default()
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
