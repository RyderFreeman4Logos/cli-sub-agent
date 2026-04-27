use super::{
    PostExecGateCommandOutcome, PostExecGateOutcome, maybe_run_post_exec_gate_with_runner,
};
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
            allow_base_branch_commit: false,
            post_exec_gate: gate,
        },
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
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
        assert!(status.success(), "git command failed: {:?}", args);
    };

    run(&["init", "--initial-branch", "main"]);
    run(&["config", "user.name", "CSA Test"]);
    run(&["config", "user.email", "csa-test@example.com"]);
    std::fs::write(project_root.join("tracked.txt"), "initial\n").expect("write tracked file");
    run(&["add", "tracked.txt"]);
    run(&["commit", "-m", "initial"]);
}

#[tokio::test]
async fn post_exec_gate_passes_when_command_succeeds() {
    let project_dir = tempdir().unwrap();
    init_clean_git_repo(project_dir.path());
    std::fs::write(project_dir.path().join("tracked.txt"), "changed\n").unwrap();

    let calls = Arc::new(Mutex::new(Vec::new()));
    let config = project_config_with_gate(PostExecGateConfig::default());
    let outcome = maybe_run_post_exec_gate_with_runner(
        project_dir.path(),
        "Implement the fix in tracked.txt",
        Some("01TESTPOSTEXECGATEPASS0000000"),
        Some(&config),
        {
            let calls = Arc::clone(&calls);
            move |command, cwd, timeout_seconds| {
                let calls = Arc::clone(&calls);
                let command = command.to_string();
                let cwd = cwd.to_path_buf();
                Box::pin(async move {
                    calls.lock().unwrap().push((command, cwd, timeout_seconds));
                    Ok(PostExecGateCommandOutcome::Exited(Some(0)))
                })
            }
        },
    )
    .await
    .expect("gate should pass");

    assert_eq!(outcome, PostExecGateOutcome::Passed);
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "just pre-commit");
    assert_eq!(calls[0].1, project_dir.path());
    assert_eq!(calls[0].2, 600);
}

#[tokio::test]
async fn post_exec_gate_failure_returns_structured_diagnostic() {
    let project_dir = tempdir().unwrap();
    init_clean_git_repo(project_dir.path());
    std::fs::write(project_dir.path().join("tracked.txt"), "changed\n").unwrap();

    let config = project_config_with_gate(PostExecGateConfig {
        command: "cargo test".to_string(),
        ..Default::default()
    });
    let err = maybe_run_post_exec_gate_with_runner(
        project_dir.path(),
        "Implement the fix in tracked.txt",
        Some("01TESTPOSTEXECGATEFAIL0000000"),
        Some(&config),
        |_command, _cwd, _timeout_seconds| {
            Box::pin(async { Ok(PostExecGateCommandOutcome::Exited(Some(3))) })
        },
    )
    .await
    .expect_err("gate failure should bubble up");

    let rendered = format!("{err:#}");
    assert!(rendered.contains("csa: post-exec gate failed (exit=3)."));
    assert!(rendered.contains("gate command: cargo test"));
    assert!(rendered.contains(&format!("cwd: {}", project_dir.path().display())));
    assert!(rendered.contains("employee session: 01TESTPOSTEXECGATEFAIL0000000"));
    assert!(rendered.contains("branch: main"));
    assert!(rendered.contains("v1 gate does NOT auto-retry"));
}

#[tokio::test]
async fn post_exec_gate_skips_when_worktree_is_clean() {
    let project_dir = tempdir().unwrap();
    init_clean_git_repo(project_dir.path());

    let config = project_config_with_gate(PostExecGateConfig::default());
    let outcome = maybe_run_post_exec_gate_with_runner(
        project_dir.path(),
        "Implement the fix in tracked.txt",
        Some("01TESTPOSTEXECGATESKIP0000000"),
        Some(&config),
        |_command, _cwd, _timeout_seconds| {
            Box::pin(async move {
                panic!("runner must not execute when worktree is clean");
            })
        },
    )
    .await
    .expect("clean worktree should skip gate");

    assert_eq!(outcome, PostExecGateOutcome::Skipped);
}

#[tokio::test]
async fn post_exec_gate_skips_review_and_debate_prompts() {
    let project_dir = tempdir().unwrap();
    init_clean_git_repo(project_dir.path());
    std::fs::write(project_dir.path().join("tracked.txt"), "changed\n").unwrap();

    let config = project_config_with_gate(PostExecGateConfig::default());
    for prompt in ["# REVIEW:\nInspect the diff", "# DEBATE:\nArgue both sides"] {
        let outcome = maybe_run_post_exec_gate_with_runner(
            project_dir.path(),
            prompt,
            Some("01TESTPOSTEXECGATEREVIEW00000"),
            Some(&config),
            |_command, _cwd, _timeout_seconds| {
                Box::pin(async move {
                    panic!("runner must not execute for review/debate prompts");
                })
            },
        )
        .await
        .expect("review/debate prompts should skip gate");

        assert_eq!(outcome, PostExecGateOutcome::Skipped);
    }
}
