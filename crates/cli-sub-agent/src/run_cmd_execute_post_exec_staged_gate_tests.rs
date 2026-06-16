use super::{
    PostExecGateCommandOutcome, PostExecGateOutcome, maybe_run_post_exec_gate_with_runner,
};
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_config::{PostExecGateConfig, ProjectConfig, ProjectMeta, ResourcesConfig, RunConfig};
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

fn git_stdout(project_root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .output()
        .expect("run git");
    assert!(output.status.success(), "git command failed: {:?}", args);
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn init_clean_git_repo(project_root: &Path) {
    run_git(project_root, &["init", "--initial-branch", "main"]);
    run_git(project_root, &["config", "user.name", "CSA Test"]);
    run_git(
        project_root,
        &["config", "user.email", "csa-test@example.com"],
    );
    std::fs::write(project_root.join("tracked.txt"), "initial\n").expect("write tracked file");
    run_git(project_root, &["add", "tracked.txt"]);
    run_git(project_root, &["commit", "-m", "initial"]);
}

#[tokio::test]
async fn post_exec_gate_stages_changed_paths_in_temporary_index() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    init_clean_git_repo(project_dir.path());
    std::fs::write(project_dir.path().join("tracked.txt"), "changed\n").unwrap();

    let config = project_config_with_gate(PostExecGateConfig::default());
    let changed_paths = vec!["tracked.txt".to_string()];
    let outcome = maybe_run_post_exec_gate_with_runner(
        project_dir.path(),
        "Modify tracked.txt",
        Some("01TESTPOSTEXECGATESTAGED000"),
        Some(&config),
        Some(&changed_paths),
        Some(HashMap::from([(
            "CARGO_BUILD_JOBS".to_string(),
            "1".to_string(),
        )])),
        |_command, cwd, _timeout_seconds, extra_env| {
            let cwd = cwd.to_path_buf();
            Box::pin(async move {
                let extra_env = extra_env.expect("gate should run with temp index env");
                let index = extra_env
                    .get("GIT_INDEX_FILE")
                    .expect("GIT_INDEX_FILE must be set for staged-scope gates");
                assert!(
                    Path::new(index).exists(),
                    "temp index must exist while gate runs"
                );
                assert_eq!(extra_env.get("CARGO_BUILD_JOBS"), Some(&"1".to_string()));
                let output = std::process::Command::new("git")
                    .arg("diff")
                    .args(["--cached", "--name-only"])
                    .current_dir(&cwd)
                    .envs(&extra_env)
                    .output()
                    .expect("inspect temp staged diff");
                assert!(output.status.success(), "git diff --cached should succeed");
                assert_eq!(String::from_utf8_lossy(&output.stdout), "tracked.txt\n");
                Ok(PostExecGateCommandOutcome::exited(Some(0)))
            })
        },
    )
    .await
    .expect("gate should pass");

    assert_eq!(outcome, PostExecGateOutcome::Passed);
    assert_eq!(
        git_stdout(project_dir.path(), &["diff", "--cached", "--name-only"]),
        "",
        "post-exec gate must not stage paths in the real index"
    );
}

#[tokio::test]
async fn post_exec_gate_stages_changed_paths_as_literal_pathspecs() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    init_clean_git_repo(project_dir.path());
    std::fs::write(project_dir.path().join(":(glob)*"), "magic\n").unwrap();
    std::fs::write(project_dir.path().join("unrelated.txt"), "unrelated\n").unwrap();

    let config = project_config_with_gate(PostExecGateConfig::default());
    let changed_paths = vec![":(glob)*".to_string()];
    let outcome = maybe_run_post_exec_gate_with_runner(
        project_dir.path(),
        "Create pathspec-magic filename",
        Some("01TESTPOSTEXECGATELITERAL00"),
        Some(&config),
        Some(&changed_paths),
        None,
        |_command, cwd, _timeout_seconds, extra_env| {
            let cwd = cwd.to_path_buf();
            Box::pin(async move {
                let extra_env = extra_env.expect("gate should run with temp index env");
                let output = std::process::Command::new("git")
                    .arg("diff")
                    .args(["--cached", "--name-only"])
                    .current_dir(&cwd)
                    .envs(&extra_env)
                    .output()
                    .expect("inspect temp staged diff");
                assert!(output.status.success(), "git diff --cached should succeed");
                let staged = String::from_utf8_lossy(&output.stdout);
                assert_eq!(staged, ":(glob)*\n");
                Ok(PostExecGateCommandOutcome::exited(Some(0)))
            })
        },
    )
    .await
    .expect("gate should pass");

    assert_eq!(outcome, PostExecGateOutcome::Passed);
    assert_eq!(
        git_stdout(project_dir.path(), &["diff", "--cached", "--name-only"]),
        "",
        "post-exec gate must not stage literal pathspec paths in the real index"
    );
}

#[tokio::test]
async fn post_exec_gate_tolerates_deleted_never_tracked_changed_paths() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    init_clean_git_repo(project_dir.path());
    let scratch = project_dir.path().join("scratch.tmp");
    std::fs::write(&scratch, "scratch\n").unwrap();
    std::fs::remove_file(&scratch).unwrap();
    let config = project_config_with_gate(PostExecGateConfig::default());
    let changed_paths = vec!["scratch.tmp".to_string()];
    let outcome = maybe_run_post_exec_gate_with_runner(
        project_dir.path(),
        "Delete pre-existing untracked scratch file",
        Some("01TESTPOSTEXECGATEDELETED0"),
        Some(&config),
        Some(&changed_paths),
        None,
        |_command, cwd, _timeout_seconds, extra_env| {
            let cwd = cwd.to_path_buf();
            Box::pin(async move {
                let extra_env = extra_env.expect("gate should still run with temp index env");
                let output = std::process::Command::new("git")
                    .arg("diff")
                    .args(["--cached", "--name-only"])
                    .current_dir(&cwd)
                    .envs(&extra_env)
                    .output()
                    .expect("inspect temp staged diff");
                assert!(output.status.success(), "git diff --cached should succeed");
                assert_eq!(String::from_utf8_lossy(&output.stdout), "");
                Ok(PostExecGateCommandOutcome::exited(Some(0)))
            })
        },
    )
    .await
    .expect("gate should tolerate a deleted never-tracked changed path");
    assert_eq!(outcome, PostExecGateOutcome::Passed);
    assert_eq!(
        git_stdout(project_dir.path(), &["diff", "--cached", "--name-only"]),
        "",
        "post-exec gate must not stage deleted never-tracked paths in the real index"
    );
}
