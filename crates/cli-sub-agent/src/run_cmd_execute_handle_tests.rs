use super::PostExecGateApplyOptions;
use super::handle::record_run_dirty_then_apply_post_exec_gate;
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_config::{PostExecGateConfig, ProjectConfig, ProjectMeta, ResourcesConfig, RunConfig};
use csa_session::{SessionResult, create_session_fresh, load_result, save_result};
use std::collections::HashMap;
use std::path::Path;
use tempfile::tempdir;

fn project_config_with_gate(gate: PostExecGateConfig) -> ProjectConfig {
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

fn init_clean_git_repo(project_root: &Path) {
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .status()
            .expect("run git");
        assert!(status.success(), "git command failed: {args:?}");
    };
    run(&["init", "--initial-branch", "main"]);
    run(&["config", "user.name", "CSA Test"]);
    run(&["config", "user.email", "csa-test@example.com"]);
    std::fs::write(project_root.join("tracked.txt"), "initial\n").expect("write tracked file");
    run(&["add", "tracked.txt"]);
    run(&["commit", "-m", "initial"]);
}

fn write_success_result_for(project_root: &Path, session_id: &str) {
    let now = chrono::Utc::now();
    save_result(
        project_root,
        session_id,
        &SessionResult {
            post_exec_gate: None,
            status: "success".to_string(),
            exit_code: 0,
            summary: "writer completed".to_string(),
            tool: "codex".to_string(),
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
            started_at: now,
            completed_at: now,
            events_count: 0,
            artifacts: Vec::new(),
            ..Default::default()
        },
    )
    .expect("write success result");
}

#[tokio::test]
async fn handle_run_sequencing_runs_failing_gate_after_require_commit_fatal() {
    let project_dir = tempdir().expect("temp project");
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    init_clean_git_repo(project_dir.path());
    let session_id = create_session_fresh(
        project_dir.path(),
        Some("handle-run gate eligibility"),
        None,
        Some("codex"),
    )
    .expect("create session")
    .meta_session_id;
    write_success_result_for(project_dir.path(), &session_id);
    std::fs::write(project_dir.path().join("tracked.txt"), "dirty\n").expect("dirty tracked file");

    let mut gate = PostExecGateConfig::default();
    gate.command = "printf 'simulated gate failure\\n' >&2; exit 37".to_string();
    let config = project_config_with_gate(gate);
    let changed_paths = vec!["tracked.txt".to_string()];
    let mut execution = csa_process::ExecutionResult {
        summary: "writer completed".to_string(),
        exit_code: 0,
        model_completed: Some(true),
        terminal_reason: Some("end_turn".to_string()),
        ..Default::default()
    };

    let err = record_run_dirty_then_apply_post_exec_gate(
        project_dir.path(),
        "modify tracked.txt",
        Some(&session_id),
        Some(&config),
        &mut execution,
        Some(&changed_paths),
        Some(false),
        true,
        PostExecGateApplyOptions {
            changed_paths: Some(&changed_paths),
            extra_env: None,
            no_post_exec_gate: false,
            planning_only: false,
        },
    )
    .await
    .expect_err("failing gate must remain fatal after require-commit failure");

    assert!(err.to_string().contains("post-exec gate failed"));
    assert_eq!(
        execution.exit_code, 1,
        "require-commit made the writer result fatal"
    );
    let persisted = load_result(project_dir.path(), &session_id)
        .expect("load result")
        .expect("persisted result");
    assert_eq!(persisted.status, "failure");
    assert!(persisted.summary.starts_with("POST-EXEC GATE FAILED"));
    let report = persisted
        .post_exec_gate
        .as_ref()
        .expect("[post_exec_gate] evidence must be persisted");
    assert_eq!(report.exit_code, 37);
    assert!(report.output_tail.contains("simulated gate failure"));
    assert!(
        persisted.require_commit_recovery.is_some(),
        "gate evidence must retain require-commit recovery"
    );
}
