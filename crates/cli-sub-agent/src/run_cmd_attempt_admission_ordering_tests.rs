use super::{RunLoopRequest, execute_run_loop, test_events};
use crate::run_helpers_branch_guard::BranchGuardRuntime;
use crate::run_resource_overrides::RunResourceOverrides;
use crate::startup_env::StartupSubtreeEnv;
use crate::test_env_lock::ScopedEnvVarRestore;
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_config::GlobalConfig;
use csa_core::types::{OutputFormat, ToolName, ToolSelectionStrategy};
use std::time::Instant;

#[tokio::test]
async fn run_loop_acquires_slot_before_host_memory_admission() {
    let project_dir = tempfile::tempdir().expect("temp project");
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let _tools_available =
        ScopedEnvVarRestore::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let _resource_capability =
        ScopedEnvVarRestore::set(csa_resource::sandbox::TEST_ASSUME_CGROUP_V2_ENV, "1");
    let mut config = crate::review_cmd::tests::project_config_with_enabled_tools(&["codex"]);
    config.resources.memory_max_mb = Some(10_000);
    config.resources.min_free_memory_mb = u64::MAX;
    config.resources.soft_limit_percent = Some(90);
    let global_config = GlobalConfig::default();
    let model_catalog = csa_config::EffectiveModelCatalog::shipped().expect("shipped catalog");
    let startup_env = StartupSubtreeEnv::default();
    let recorder = test_events::EventRecorder::start();

    let result = execute_run_loop(RunLoopRequest {
        strategy: ToolSelectionStrategy::Explicit(ToolName::Codex),
        initial_tool: ToolName::Codex,
        initial_model_spec: None,
        user_model_spec_explicit: false,
        subtree_model_pin_spec: None,
        subtree_model_pin_force_ignore_tier_setting: false,
        initial_model: None,
        runtime_fallback_candidates: Vec::new(),
        project_root: project_dir.path(),
        config: Some(&config),
        global_config: &global_config,
        model_catalog: &model_catalog,
        prompt_text: "ordering regression",
        skill: None,
        skill_session_tag: None,
        description: None,
        parent: None,
        output_format: OutputFormat::Text,
        stream_mode: csa_process::StreamMode::BufferOnly,
        thinking: None,
        force: false,
        force_override_user_config: false,
        force_ignore_tier_setting: false,
        no_failover: true,
        fast_but_more_cost: false,
        build_jobs: None,
        resource_overrides: RunResourceOverrides::absent(),
        wait: false,
        idle_timeout_seconds: 120,
        cli_idle_timeout: None,
        cli_initial_response_timeout: None,
        no_idle_timeout: false,
        run_timeout_seconds: None,
        run_started_at: Instant::now(),
        is_fork: false,
        is_auto_seed_fork: false,
        caller_fork_resolution: None,
        ephemeral: false,
        fork_call: false,
        session_arg: None,
        effective_session_arg: None,
        tier_auto_select: false,
        failover_on_crash_enabled: false,
        resolved_tier_name: None,
        tier_failover_tool_filter: None,
        context_load_options: None,
        memory_injection: Default::default(),
        pre_session_hook: None,
        task_needs_edit: None,
        no_fs_sandbox: false,
        allow_user_daemon_ipc: false,
        allow_git_push: false,
        error_marker_scan_override: None,
        no_hook_bypass_scan: false,
        extra_writable: Vec::new(),
        extra_readable: Vec::new(),
        branch_guard: BranchGuardRuntime::for_run(false, Some(&config), &global_config),
        startup_env: &startup_env,
    })
    .await;
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("host-memory admission must stop the run before tool execution"),
    };

    assert!(
        format!("{error:#}").contains("run preflight for writer tool 'codex'"),
        "expected the post-slot host-memory admission error: {error:#}"
    );
    assert_eq!(
        recorder.events(),
        [
            test_events::AttemptEvent::StaticSoftLimit,
            test_events::AttemptEvent::SlotAcquired,
            test_events::AttemptEvent::HostMemoryAfterSlot,
        ]
    );
}
