use super::{
    PostExecGateCommandOutcome, PostExecGateOutcome, maybe_run_post_exec_gate_with_runner,
};
use crate::test_env_lock::ScopedEnvVarRestore;
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
async fn post_exec_gate_normalizes_readonly_usr_local_rust_env() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let _target = ScopedEnvVarRestore::unset(csa_core::env::CARGO_TARGET_DIR_ENV_KEY);
    init_clean_git_repo(project_dir.path());
    std::fs::write(project_dir.path().join("tracked.txt"), "changed\n").unwrap();
    let home = project_dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    let config = project_config_with_gate(PostExecGateConfig::default());
    let changed_paths = vec!["tracked.txt".to_string()];
    let expected_target = project_dir.path().join("target");
    let expected_install_root = expected_target.join("cargo-install-root");
    let extra_env = HashMap::from([
        ("CARGO_BUILD_JOBS".to_string(), "1".to_string()),
        ("HOME".to_string(), home.to_string_lossy().into_owned()),
        (
            csa_core::env::CARGO_HOME_ENV_KEY.to_string(),
            "/usr/local".to_string(),
        ),
        (
            csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY.to_string(),
            "/usr/local".to_string(),
        ),
    ]);
    let outcome = maybe_run_post_exec_gate_with_runner(
        project_dir.path(),
        "Modify tracked.txt",
        Some("01TESTPOSTEXECGATERUSTENV0"),
        Some(&config),
        Some(&changed_paths),
        Some(extra_env),
        move |_command, _cwd, _timeout_seconds, extra_env| {
            Box::pin(async move {
                let extra_env = extra_env.expect("gate should receive normalized env");
                assert!(extra_env.contains_key("GIT_INDEX_FILE"));
                assert_eq!(extra_env.get("CARGO_BUILD_JOBS"), Some(&"1".to_string()));
                assert_eq!(
                    extra_env
                        .get(csa_core::env::CARGO_TARGET_DIR_ENV_KEY)
                        .map(String::as_str),
                    Some(expected_target.to_str().unwrap())
                );
                assert_eq!(
                    extra_env
                        .get(csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY)
                        .map(String::as_str),
                    Some(expected_install_root.to_str().unwrap())
                );
                let cargo_home = extra_env
                    .get(csa_core::env::CARGO_HOME_ENV_KEY)
                    .expect("CARGO_HOME should be normalized");
                assert_ne!(cargo_home, "/usr/local");
                assert!(
                    !csa_core::env::rust_state_path_needs_session_override(Path::new(cargo_home)),
                    "post-exec gate CARGO_HOME must not target read-only /usr/local: {cargo_home}"
                );
                Ok(PostExecGateCommandOutcome::exited(Some(0)))
            })
        },
    )
    .await
    .expect("gate should pass");

    assert_eq!(outcome, PostExecGateOutcome::Passed);
}

#[tokio::test]
async fn post_exec_gate_pins_safe_ambient_cargo_paths_to_project_target() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    init_clean_git_repo(project_dir.path());
    std::fs::write(project_dir.path().join("tracked.txt"), "changed\n").unwrap();
    let ambient_target = project_dir.path().join("ambient-target");
    let ambient_install_root = project_dir.path().join("ambient-cargo-install-root");
    let _target =
        ScopedEnvVarRestore::set(csa_core::env::CARGO_TARGET_DIR_ENV_KEY, &ambient_target);
    let _install_root = ScopedEnvVarRestore::set(
        csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY,
        &ambient_install_root,
    );

    let config = project_config_with_gate(PostExecGateConfig::default());
    let changed_paths = vec!["tracked.txt".to_string()];
    let expected_target = project_dir.path().join("target");
    let expected_install_root = expected_target.join("cargo-install-root");
    let outcome = maybe_run_post_exec_gate_with_runner(
        project_dir.path(),
        "Modify tracked.txt",
        Some("01TESTPOSTEXECGATEAMBSAFE0"),
        Some(&config),
        Some(&changed_paths),
        Some(HashMap::from([(
            "CARGO_BUILD_JOBS".to_string(),
            "1".to_string(),
        )])),
        move |_command, _cwd, _timeout_seconds, extra_env| {
            let expected_target = expected_target.clone();
            let expected_install_root = expected_install_root.clone();
            Box::pin(async move {
                let extra_env = extra_env.expect("gate should receive temp index env");
                assert!(extra_env.contains_key("GIT_INDEX_FILE"));
                assert_eq!(extra_env.get("CARGO_BUILD_JOBS"), Some(&"1".to_string()));
                assert_eq!(
                    extra_env
                        .get(csa_core::env::CARGO_TARGET_DIR_ENV_KEY)
                        .map(String::as_str),
                    Some(expected_target.to_str().unwrap())
                );
                assert_eq!(
                    extra_env
                        .get(csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY)
                        .map(String::as_str),
                    Some(expected_install_root.to_str().unwrap())
                );
                let output = std::process::Command::new("sh")
                    .arg("-c")
                    .arg("printf '%s\\n%s\\n' \"$CARGO_TARGET_DIR\" \"$CARGO_INSTALL_ROOT\"")
                    .envs(&extra_env)
                    .output()
                    .expect("capture post-exec gate cargo env");
                assert!(
                    output.status.success(),
                    "env capture should succeed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                let captured = String::from_utf8_lossy(&output.stdout);
                let lines = captured.lines().collect::<Vec<_>>();
                assert_eq!(lines.len(), 2);
                assert_eq!(lines[0], expected_target.to_str().unwrap());
                assert_eq!(lines[1], expected_install_root.to_str().unwrap());
                Ok(PostExecGateCommandOutcome::exited(Some(0)))
            })
        },
    )
    .await
    .expect("gate should pass");

    assert_eq!(outcome, PostExecGateOutcome::Passed);
}

#[tokio::test]
async fn post_exec_gate_normalizes_ambient_usr_local_cargo_paths() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    init_clean_git_repo(project_dir.path());
    std::fs::write(project_dir.path().join("tracked.txt"), "changed\n").unwrap();
    let _target = ScopedEnvVarRestore::set(csa_core::env::CARGO_TARGET_DIR_ENV_KEY, "/usr/local");
    let _install_root =
        ScopedEnvVarRestore::set(csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY, "/usr/local");

    let config = project_config_with_gate(PostExecGateConfig::default());
    let changed_paths = vec!["tracked.txt".to_string()];
    let expected_target = project_dir.path().join("target");
    let expected_install_root = expected_target.join("cargo-install-root");
    let outcome = maybe_run_post_exec_gate_with_runner(
        project_dir.path(),
        "Modify tracked.txt",
        Some("01TESTPOSTEXECGATEAMBLOCAL"),
        Some(&config),
        Some(&changed_paths),
        Some(HashMap::from([(
            "CARGO_BUILD_JOBS".to_string(),
            "1".to_string(),
        )])),
        move |_command, _cwd, _timeout_seconds, extra_env| {
            Box::pin(async move {
                let extra_env = extra_env.expect("gate should receive normalized env");
                assert!(extra_env.contains_key("GIT_INDEX_FILE"));
                assert_eq!(
                    extra_env
                        .get(csa_core::env::CARGO_TARGET_DIR_ENV_KEY)
                        .map(String::as_str),
                    Some(expected_target.to_str().unwrap())
                );
                assert_eq!(
                    extra_env
                        .get(csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY)
                        .map(String::as_str),
                    Some(expected_install_root.to_str().unwrap())
                );
                Ok(PostExecGateCommandOutcome::exited(Some(0)))
            })
        },
    )
    .await
    .expect("gate should pass");

    assert_eq!(outcome, PostExecGateOutcome::Passed);
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
