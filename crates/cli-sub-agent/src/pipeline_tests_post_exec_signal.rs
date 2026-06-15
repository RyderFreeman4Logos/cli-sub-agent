use super::*;

#[tokio::test]
async fn process_execution_result_persists_signal_kill_hint() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new(&tmp).await;
    let project_root = tmp.path();
    let executor = csa_executor::Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: csa_executor::CodexRuntimeMetadata::current(),
    };
    let hooks_config = csa_hooks::HooksConfig::default();
    let mut session =
        create_session(project_root, Some("signal"), None, Some("codex")).expect("create session");
    let session_dir = get_session_dir(project_root, &session.meta_session_id).expect("session dir");
    let memory_registry_key =
        csa_resource::memory_monitor::soft_limit_diagnostic_path_for_session_dir(&session_dir)
            .expect("CSA-owned memory diagnostic path");
    assert!(
        !memory_registry_key.starts_with(&session_dir),
        "memory-soft-limit evidence must live outside child-writable session dir"
    );
    let memory_event = csa_resource::memory_monitor::MemorySoftLimitKillDiagnostic {
        kill_hint: csa_resource::memory_monitor::MEMORY_SOFT_LIMIT_KILL_HINT.to_string(),
        signal: libc::SIGTERM,
        current_mb: 9216,
        threshold_mb: 8601,
        memory_max_mb: 12_288,
        soft_limit_percent: 70,
        scope_name: "csa-codex-01KTEST.scope".to_string(),
    };
    csa_resource::memory_monitor::record_soft_limit_diagnostic_evidence(
        &memory_registry_key,
        &memory_event,
    );
    assert!(
        !memory_registry_key.exists(),
        "result diagnostics should use registry evidence without a disk artifact"
    );
    let ctx = PostExecContext {
        executor: &executor,
        prompt: "test prompt",
        effective_prompt: "test prompt",
        task_type: Some("run"),
        readonly_project_root: false,
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
        transcript_artifacts: Vec::new(),
        changed_paths: Vec::new(),
        pre_exec_snapshot: None,
        has_tool_calls: true,
        turn_count: 1,
        output_tokens: None,
        sa_mode: false,
    };
    let mut result = csa_process::ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "killed by signal 9".to_string(),
        exit_code: 137,
        peak_memory_mb: None,
        terminal_reason: Some("signal".to_string()),
        ..Default::default()
    };

    process_execution_result(ctx, &mut session, &mut result)
        .await
        .expect("process result");

    let raw = fs::read_to_string(session_dir.join("result.toml")).expect("read result");
    let value = toml::from_str::<toml::Value>(&raw).expect("parse result");
    assert_eq!(
        value.get("kill_hint").and_then(toml::Value::as_str),
        Some("memory_soft_limit")
    );
    let diagnostics = value
        .get("kill_diagnostics")
        .and_then(toml::Value::as_table)
        .expect("kill_diagnostics should be persisted");
    assert_eq!(
        diagnostics.get("source").and_then(toml::Value::as_str),
        Some("memory_soft_limit")
    );
    assert_eq!(
        diagnostics
            .get("current_mb")
            .and_then(toml::Value::as_integer),
        Some(9216)
    );
    assert_eq!(
        value.get("last_item").and_then(toml::Value::as_str),
        Some("killed by signal 9")
    );

    let loaded = csa_session::load_result(project_root, &session.meta_session_id)
        .expect("load result")
        .expect("result should exist");
    assert_eq!(loaded.kill_hint.as_deref(), Some("memory_soft_limit"));
    assert_eq!(
        loaded
            .kill_diagnostics
            .as_ref()
            .map(|diagnostics| diagnostics.source.as_str()),
        Some("memory_soft_limit")
    );
    assert_eq!(loaded.last_item.as_deref(), Some("killed by signal 9"));
    assert_eq!(
        session.termination_reason.as_deref(),
        Some("memory_soft_limit")
    );
    let tool_state = session.tools.get("codex").expect("codex tool state");
    assert_eq!(tool_state.last_exit_code, 137);
    assert!(tool_state.last_action_summary.contains("memory soft limit"));
}
