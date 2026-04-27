use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

use serial_test::serial;

fn csa_cmd(tmp: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_csa"));
    cmd.env("HOME", tmp)
        .env("XDG_STATE_HOME", tmp.join(".local/state"))
        .env("XDG_CONFIG_HOME", tmp.join(".config"))
        .env("TOKIO_WORKER_THREADS", "1")
        .env_remove("CSA_DEPTH")
        .env_remove("CSA_SESSION_ID")
        .env_remove("CSA_SESSION_DIR");
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

fn run_csa_with_missing_tool_mode(
    tmp: &Path,
    project_root: &Path,
    extra_args: &[&str],
    no_daemon: bool,
) -> Output {
    let mut cmd = csa_cmd(tmp);
    cmd.arg("run");
    if no_daemon {
        cmd.arg("--no-daemon");
    }
    cmd.args(["--sa-mode", "true", "--tool", "missing-tool-alias"])
        .args(extra_args)
        .arg("inspect repository")
        .current_dir(project_root)
        .output()
        .expect("csa run should execute")
}

fn run_csa_with_missing_tool(tmp: &Path, project_root: &Path, extra_args: &[&str]) -> Output {
    run_csa_with_missing_tool_mode(tmp, project_root, extra_args, true)
}

fn run_csa_daemon_with_missing_tool(
    tmp: &Path,
    project_root: &Path,
    extra_args: &[&str],
) -> Output {
    run_csa_with_missing_tool_mode(tmp, project_root, extra_args, false)
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
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stdout_without_guards = strip_caller_sa_guard_blocks(&stdout);
    assert!(
        stdout_without_guards.trim().is_empty(),
        "refusal should not write success data to stdout: {}",
        stdout
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
    assert!(
        !stderr.contains("CSA:SESSION_STARTED"),
        "refusal should happen before daemon spawn: {stderr}"
    );
}

fn strip_caller_sa_guard_blocks(text: &str) -> String {
    let mut cleaned = String::new();
    let mut in_guard = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("<csa-caller-sa-guard") {
            in_guard = true;
            continue;
        }
        if trimmed == "</csa-caller-sa-guard>" {
            in_guard = false;
            continue;
        }
        if !in_guard {
            cleaned.push_str(line);
            cleaned.push('\n');
        }
    }
    cleaned
}

fn assert_branch_guard_allowed_to_later_error(output: &Output) {
    assert_ne!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("refusing to run on protected branch"),
        "guard should not refuse: {stderr}"
    );
}

fn assert_daemon_spawned_and_cleanup(output: &Output, tmp: &Path, project_root: &Path) {
    assert_eq!(
        output.status.code(),
        Some(0),
        "daemon parent should exit 0\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stdout_without_guards = strip_caller_sa_guard_blocks(&stdout);
    let session_id = stdout_without_guards.trim();
    assert!(
        session_id.len() == 26 && session_id.chars().all(|ch| ch.is_ascii_alphanumeric()),
        "stdout should contain only the daemon session id: {stdout}"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("CSA:SESSION_STARTED"),
        "stderr should contain daemon start directive: {stderr}"
    );
    assert!(
        !stderr.contains("refusing to run on protected branch"),
        "guard should not refuse: {stderr}"
    );

    let session_dir = daemon_session_dir(tmp, project_root, session_id);
    wait_for_daemon_exit_or_cleanup(&session_dir);
}

fn daemon_session_dir(tmp: &Path, project_root: &Path, session_id: &str) -> PathBuf {
    let canonical = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let normalized = canonical
        .to_string_lossy()
        .trim_start_matches('/')
        .replace('/', std::path::MAIN_SEPARATOR_STR);
    tmp.join(".local/state")
        .join("cli-sub-agent")
        .join(normalized)
        .join("sessions")
        .join(session_id)
}

fn wait_for_daemon_exit_or_cleanup(session_dir: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if session_dir.join("daemon-completion.toml").exists()
            || !csa_process::ToolLiveness::daemon_pid_is_alive(session_dir)
        {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }

    terminate_daemon_process_group(session_dir);
    panic!(
        "daemon did not exit before timeout; session_dir={}",
        session_dir.display()
    );
}

#[cfg(unix)]
fn terminate_daemon_process_group(session_dir: &Path) {
    let Some(pid) = csa_process::ToolLiveness::daemon_pid_for_signal(session_dir) else {
        return;
    };
    let pgid = -(pid as libc::pid_t);
    unsafe {
        libc::kill(pgid, libc::SIGTERM);
    }
    thread::sleep(Duration::from_millis(200));
    if csa_process::ToolLiveness::daemon_pid_is_alive(session_dir) {
        unsafe {
            libc::kill(pgid, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
fn terminate_daemon_process_group(_session_dir: &Path) {}

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
fn daemon_run_on_main_without_bypass_refuses_before_spawn() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_git_repo(tmp.path(), "main");

    let output = run_csa_daemon_with_missing_tool(tmp.path(), tmp.path(), &[]);

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
fn daemon_run_on_feature_branch_spawns_before_later_tool_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_git_repo(tmp.path(), "main");
    run_git(tmp.path(), &["checkout", "-b", "feat/branch-guard"]);

    let output = run_csa_daemon_with_missing_tool(tmp.path(), tmp.path(), &[]);

    assert_daemon_spawned_and_cleanup(&output, tmp.path(), tmp.path());
}

#[test]
fn run_with_cd_subdirectory_on_main_refuses_before_tool_execution() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_git_repo(tmp.path(), "main");
    std::fs::create_dir(tmp.path().join("crates")).expect("create subdirectory");

    let output = run_csa_with_missing_tool(tmp.path(), tmp.path(), &["--cd", "crates"]);

    assert_branch_guard_refused(&output);
}

#[test]
fn run_with_cd_subdirectory_on_feature_branch_allows_guard_to_later_tool_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_git_repo(tmp.path(), "main");
    run_git(tmp.path(), &["checkout", "-b", "feat/branch-guard"]);
    std::fs::create_dir(tmp.path().join("crates")).expect("create subdirectory");

    let output = run_csa_with_missing_tool(tmp.path(), tmp.path(), &["--cd", "crates"]);

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
fn daemon_run_on_main_with_cli_bypass_spawns_before_later_tool_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_git_repo(tmp.path(), "main");

    let output =
        run_csa_daemon_with_missing_tool(tmp.path(), tmp.path(), &["--allow-base-branch-commit"]);

    assert_daemon_spawned_and_cleanup(&output, tmp.path(), tmp.path());
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
fn forged_child_session_env_tuple_refuses_on_protected_branch() {
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
        .expect("create parent session");
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

    assert_branch_guard_refused(&output);
}

#[test]
#[serial]
fn daemon_run_with_forged_child_session_env_tuple_refuses_before_spawn() {
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
        .expect("create parent session");
    let session_dir =
        csa_session::get_session_dir(tmp.path(), &session.meta_session_id).expect("session dir");

    let output = csa_cmd(tmp.path())
        .args([
            "run",
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

    assert_branch_guard_refused(&output);
}

#[test]
#[serial]
fn forged_child_session_env_without_session_dir_refuses() {
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
        .expect("create parent session");

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
        .env_remove("CSA_SESSION_DIR")
        .current_dir(tmp.path())
        .output()
        .expect("csa run should execute");

    assert_branch_guard_refused(&output);
}

#[test]
fn ephemeral_does_not_bypass_branch_guard() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_git_repo(tmp.path(), "main");

    let output = run_csa_with_missing_tool(tmp.path(), tmp.path(), &["--ephemeral"]);

    assert_branch_guard_refused(&output);
}
