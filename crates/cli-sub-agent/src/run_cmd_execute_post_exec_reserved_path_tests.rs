use super::{
    PostExecGateCommandOutcome, PostExecGateOutcome, maybe_run_post_exec_gate_with_runner,
};
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_config::{PostExecGateConfig, ProjectConfig, ProjectMeta, ResourcesConfig, RunConfig};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
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

fn run_git(project_root: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git command failed: {:?}", args);
}

fn init_repo_allowing_reserved_paths(project_root: &Path) {
    run_git(project_root, &["init", "--initial-branch", "main"]);
    run_git(project_root, &["config", "user.name", "CSA Test"]);
    run_git(
        project_root,
        &["config", "user.email", "csa-test@example.com"],
    );
    run_git(project_root, &["config", "core.excludesFile", "/dev/null"]);
    std::fs::write(project_root.join(".gitignore"), "cache/\nLibrary/\n").unwrap();
    std::fs::write(project_root.join("tracked.txt"), "initial\n").unwrap();
    run_git(project_root, &["add", ".gitignore", "tracked.txt"]);
    run_git(project_root, &["commit", "-m", "initial"]);
}

#[tokio::test]
async fn post_exec_gate_runs_for_tracked_reserved_state_path_delta() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    init_repo_allowing_reserved_paths(project_dir.path());
    let reserved_path = ".local/state/cli-sub-agent/src/lib.rs";
    let reserved_file = project_dir.path().join(reserved_path);
    std::fs::create_dir_all(reserved_file.parent().unwrap()).unwrap();
    std::fs::write(&reserved_file, "initial\n").unwrap();
    run_git(project_dir.path(), &["add", reserved_path]);
    run_git(
        project_dir.path(),
        &["commit", "-m", "track reserved-looking path"],
    );
    std::fs::write(&reserved_file, "changed\n").unwrap();

    let calls = Arc::new(Mutex::new(Vec::new()));
    let config = project_config_with_gate(PostExecGateConfig::default());
    let changed_paths = vec![reserved_path.to_string()];
    let outcome = maybe_run_post_exec_gate_with_runner(
        project_dir.path(),
        "Modify tracked reserved-looking path",
        Some("01TESTPOSTEXECGATERESERVED"),
        Some(&config),
        Some(&changed_paths),
        None,
        {
            let calls = Arc::clone(&calls);
            move |command, cwd, timeout_seconds, _extra_env| {
                let calls = Arc::clone(&calls);
                let command = command.to_string();
                let cwd = cwd.to_path_buf();
                Box::pin(async move {
                    calls.lock().unwrap().push((command, cwd, timeout_seconds));
                    Ok(PostExecGateCommandOutcome::exited(Some(0)))
                })
            }
        },
    )
    .await
    .expect("tracked reserved-looking path deltas should run gate");

    assert_eq!(outcome, PostExecGateOutcome::Passed);
    assert_eq!(calls.lock().unwrap().len(), 1);
}
