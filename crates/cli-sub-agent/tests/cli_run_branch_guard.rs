use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serial_test::serial;

fn csa_cmd(tmp: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_csa"));
    cmd.env("HOME", tmp)
        .env("XDG_STATE_HOME", tmp.join(".local/state"))
        .env("XDG_CONFIG_HOME", tmp.join(".config"))
        .env("TOKIO_WORKER_THREADS", "1");
    cmd
}

fn global_config_path(tmp: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        tmp.join("Library/Application Support/cli-sub-agent/config.toml")
    } else {
        tmp.join(".config/cli-sub-agent/config.toml")
    }
}

fn run_git(project_root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(project_root)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_git_repo(project_root: &Path, default_branch: &str) {
    let init_default = format!("init.defaultBranch={default_branch}");
    let output = Command::new("git")
        .args(["-c", &init_default, "init"])
        .current_dir(project_root)
        .output()
        .expect("git init should run");
    assert!(
        output.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    run_git(project_root, &["config", "user.email", "test@example.com"]);
    run_git(project_root, &["config", "user.name", "Test User"]);
    std::fs::write(project_root.join("file.txt"), "content\n").expect("write test file");
    run_git(project_root, &["add", "file.txt"]);
    run_git(project_root, &["commit", "-m", "initial"]);
}

fn run_csa_with_missing_tool(tmp: &Path, project_root: &Path, extra_args: &[&str]) -> Output {
    let mut cmd = csa_cmd(tmp);
    cmd.args([
        "run",
        "--no-daemon",
        "--sa-mode",
        "true",
        "--tool",
        "missing-tool-alias",
    ])
    .args(extra_args)
    .arg("inspect repository")
    .current_dir(project_root)
    .output()
    .expect("csa run should execute")
}

fn init_csa_project(tmp: &Path) {
    let status = csa_cmd(tmp)
        .arg("init")
        .current_dir(tmp)
        .status()
        .expect("csa init should execute");
    assert!(status.success(), "csa init should exit 0");
}

fn assert_branch_guard_refused(output: &Output) {
    assert_eq!(output.status.code(), Some(2));
    assert!(
        output.stdout.is_empty(),
        "refusal should not write success data to stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("refusing to run on protected branch"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("main"), "stderr: {stderr}");
    assert!(stderr.contains("git checkout -b"), "stderr: {stderr}");
    assert!(
        stderr.contains("--allow-base-branch-commit"),
        "stderr: {stderr}"
    );
}

fn assert_branch_guard_allowed_to_later_error(output: &Output) {
    assert_ne!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("refusing to run on protected branch"),
        "guard should not refuse: {stderr}"
    );
}

struct EnvGuard {
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl EnvGuard {
    fn set(vars: &[(&'static str, OsString)]) -> Self {
        let saved = vars
            .iter()
            .map(|(key, _)| (*key, std::env::var_os(key)))
            .collect();
        for (key, value) in vars {
            unsafe { std::env::set_var(key, value) };
        }
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..) {
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }
}

#[test]
fn run_help_displays_allow_base_branch_commit_flag() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output = csa_cmd(tmp.path())
        .args(["run", "--help"])
        .output()
        .expect("csa run --help should execute");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--allow-base-branch-commit"));
}

#[test]
fn config_show_displays_default_allow_base_branch_commit_false() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_csa_project(tmp.path());

    let output = csa_cmd(tmp.path())
        .args(["config", "show"])
        .current_dir(tmp.path())
        .output()
        .expect("csa config show should execute");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[run]"), "stdout: {stdout}");
    assert!(
        stdout.contains("allow_base_branch_commit = false"),
        "stdout: {stdout}"
    );
}

#[test]
fn run_on_main_without_bypass_refuses_before_tool_execution() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_git_repo(tmp.path(), "main");

    let output = run_csa_with_missing_tool(tmp.path(), tmp.path(), &[]);

    assert_branch_guard_refused(&output);
}

#[test]
fn run_on_feature_branch_allows_guard_to_later_tool_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_git_repo(tmp.path(), "main");
    run_git(tmp.path(), &["checkout", "-b", "feat/branch-guard"]);

    let output = run_csa_with_missing_tool(tmp.path(), tmp.path(), &[]);

    assert_branch_guard_allowed_to_later_error(&output);
}

#[test]
fn run_on_main_with_cli_bypass_allows_guard_to_later_tool_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_git_repo(tmp.path(), "main");

    let output = run_csa_with_missing_tool(tmp.path(), tmp.path(), &["--allow-base-branch-commit"]);

    assert_branch_guard_allowed_to_later_error(&output);
}

#[test]
fn run_on_main_with_project_config_bypass_still_refuses() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_git_repo(tmp.path(), "main");
    let config_path = tmp.path().join(".csa/config.toml");
    std::fs::create_dir_all(config_path.parent().expect("config dir")).expect("create config dir");
    std::fs::write(config_path, "[run]\nallow_base_branch_commit = true\n")
        .expect("write project config");

    let output = run_csa_with_missing_tool(tmp.path(), tmp.path(), &[]);

    assert_branch_guard_refused(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("project-local"), "stderr: {stderr}");
}

#[test]
fn run_on_main_with_trusted_global_config_bypass_allows_guard_to_later_tool_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_git_repo(tmp.path(), "main");
    let config_path = global_config_path(tmp.path());
    std::fs::create_dir_all(config_path.parent().expect("global config dir"))
        .expect("create global config dir");
    std::fs::write(config_path, "[run]\nallow_base_branch_commit = true\n")
        .expect("write global config");

    let output = run_csa_with_missing_tool(tmp.path(), tmp.path(), &[]);

    assert_branch_guard_allowed_to_later_error(&output);
}

#[test]
fn forged_csa_depth_without_verified_session_refuses() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_git_repo(tmp.path(), "main");

    let output = csa_cmd(tmp.path())
        .args([
            "run",
            "--no-daemon",
            "--sa-mode",
            "true",
            "--tool",
            "missing-tool-alias",
            "inspect repository",
        ])
        .env("CSA_DEPTH", "1")
        .current_dir(tmp.path())
        .output()
        .expect("csa run should execute");

    assert_branch_guard_refused(&output);
}

#[test]
#[serial]
fn verified_child_session_allows_guard_to_later_tool_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_git_repo(tmp.path(), "main");
    let state_home = tmp.path().join(".local/state");
    let config_home = tmp.path().join(".config");
    let _env = EnvGuard::set(&[
        ("HOME", tmp.path().as_os_str().to_os_string()),
        ("XDG_STATE_HOME", state_home.as_os_str().to_os_string()),
        ("XDG_CONFIG_HOME", config_home.as_os_str().to_os_string()),
    ]);
    let session = csa_session::create_session(tmp.path(), Some("parent"), None, Some("codex"))
        .expect("create verified parent session");
    let session_dir =
        csa_session::get_session_dir(tmp.path(), &session.meta_session_id).expect("session dir");

    let output = csa_cmd(tmp.path())
        .args([
            "run",
            "--no-daemon",
            "--sa-mode",
            "true",
            "--tool",
            "missing-tool-alias",
            "inspect repository",
        ])
        .env("CSA_DEPTH", "1")
        .env("CSA_SESSION_ID", &session.meta_session_id)
        .env("CSA_SESSION_DIR", session_dir)
        .current_dir(tmp.path())
        .output()
        .expect("csa run should execute");

    assert_branch_guard_allowed_to_later_error(&output);
}

#[test]
fn ephemeral_does_not_bypass_branch_guard() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_git_repo(tmp.path(), "main");

    let output = run_csa_with_missing_tool(tmp.path(), tmp.path(), &["--ephemeral"]);

    assert_branch_guard_refused(&output);
}
