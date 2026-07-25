#![cfg(unix)]

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use std::os::unix::{
    fs::{PermissionsExt, symlink},
    process::CommandExt,
};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

type DrainedPipe = Receiver<std::io::Result<Vec<u8>>>;

fn drain_pipe<R>(mut reader: R) -> DrainedPipe
where
    R: Read + Send + 'static,
{
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = sender.send(reader.read_to_end(&mut bytes).map(|_| bytes));
    });
    receiver
}

fn receive_pipe_before_deadline(
    receiver: &DrainedPipe,
    deadline: Instant,
    description: &str,
) -> Option<Vec<u8>> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    match receiver.recv_timeout(remaining) {
        Ok(Ok(bytes)) => Some(bytes),
        Ok(Err(error)) => panic!("drain {description} output: {error}"),
        Err(mpsc::RecvTimeoutError::Timeout) => None,
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            panic!("drain {description} output: pipe reader disconnected")
        }
    }
}

fn terminate_process_group(pid: u32) {
    // SAFETY: `output_with_timeout` places the direct child in a process group
    // led by `pid`, and this test harness owns that process group.
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

fn output_with_timeout(command: &mut Command, description: &str) -> Output {
    output_with_deadline(command, description, COMMAND_TIMEOUT)
}

fn output_with_deadline(
    command: &mut Command,
    description: &str,
    command_timeout: Duration,
) -> Output {
    command.process_group(0);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("spawn {description}: {error}"));
    let process_group = child.id();
    let stdout = drain_pipe(child.stdout.take().expect("piped stdout"));
    let stderr = drain_pipe(child.stderr.take().expect("piped stderr"));
    let deadline = Instant::now() + command_timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                terminate_process_group(process_group);
                let _ = child.wait();
                panic!(
                    "{description} did not finish within {} seconds",
                    command_timeout.as_secs()
                );
            }
            Err(error) => {
                terminate_process_group(process_group);
                let _ = child.wait();
                panic!("inspect {description}: {error}");
            }
        }
    };

    let Some(stdout) = receive_pipe_before_deadline(&stdout, deadline, description) else {
        terminate_process_group(process_group);
        let _ = child.wait();
        panic!(
            "{description} did not drain stdout within {} seconds",
            command_timeout.as_secs()
        );
    };
    let Some(stderr) = receive_pipe_before_deadline(&stderr, deadline, description) else {
        terminate_process_group(process_group);
        let _ = child.wait();
        panic!(
            "{description} did not drain stderr within {} seconds",
            command_timeout.as_secs()
        );
    };

    Output {
        status,
        stdout,
        stderr,
    }
}

#[test]
fn output_timeout_bounds_pipe_drain_after_direct_child_exits() {
    let mut command = Command::new("sh");
    command.args(["-c", "sleep 30 & exit 0"]);
    let started = Instant::now();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        output_with_deadline(
            &mut command,
            "direct child exited but descendant retained pipes",
            Duration::from_millis(100),
        )
    }));

    assert!(result.is_err(), "retained pipes must trip the deadline");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "pipe drain timeout must remain bounded after direct-child exit"
    );
}

fn csa_cmd(home: &Path) -> Command {
    let cargo_home = home.join(".cargo");
    let rustup_home = home.join(".rustup");
    std::fs::create_dir_all(&cargo_home).expect("create isolated cargo home");
    std::fs::create_dir_all(&rustup_home).expect("create isolated rustup home");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_csa"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("CSA_") {
            cmd.env_remove(key);
        }
    }
    cmd.env("HOME", home)
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("CARGO_HOME", cargo_home)
        .env("RUSTUP_HOME", rustup_home)
        .env("TOKIO_WORKER_THREADS", "1")
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CI");
    cmd
}

fn run_git(project_root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new("git");
    command.args(args).current_dir(project_root);
    output_with_timeout(&mut command, "git test fixture command")
}

fn require_git(project_root: &Path, args: &[&str]) {
    let output = run_git(project_root, args);
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_project(project_root: &Path) {
    std::fs::create_dir_all(project_root.join(".csa")).expect("create project config dir");
    std::fs::write(project_root.join("README.md"), "# test project\n").expect("write readme");
    std::fs::write(
        project_root.join(".csa/config.toml"),
        r#"schema_version = 1

[resources]
min_free_memory_mb = 0
memory_max_mb = 9000
soft_limit_percent = 100

[filesystem_sandbox]
enforcement_mode = "off"

[tools.codex]
enabled = true
transport = "cli"
default_model = "gpt-5.4-mini"

[run.post_exec_gate]
enabled = false
"#,
    )
    .expect("write project config");
    require_git(project_root, &["-c", "init.defaultBranch=main", "init"]);
    require_git(project_root, &["config", "user.email", "test@example.com"]);
    require_git(project_root, &["config", "user.name", "Test User"]);
    require_git(project_root, &["add", "."]);
    require_git(project_root, &["commit", "-m", "initial"]);
    require_git(
        project_root,
        &["checkout", "-b", "feat/blocked-cargo-target"],
    );
}

fn install_editing_codex(bin_dir: &Path) -> PathBuf {
    std::fs::create_dir_all(bin_dir).expect("create fake tool directory");
    let codex = bin_dir.join("codex");
    std::fs::write(
        &codex,
        r#"#!/bin/sh
printf 'provider reached\n' > provider-ran.txt
printf 'dirty edit\n' > dirty-edit.txt
printf '%s\n' \
  '{"type":"thread.started","thread_id":"blocked-cargo-target"}' \
  '{"type":"item.completed","item":{"type":"agent_message","text":"STATUS: BLOCKED; tests and commit omitted"}}' \
  '{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}'
"#,
    )
    .expect("write fake codex");
    let mut permissions = std::fs::metadata(&codex)
        .expect("fake codex metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&codex, permissions).expect("make fake codex executable");
    bin_dir.to_path_buf()
}

fn prepend_path(bin_dir: &Path) -> OsString {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin_dir.to_path_buf()];
    paths.extend(std::env::split_paths(&current));
    std::env::join_paths(paths).expect("join PATH")
}

#[test]
fn broken_external_target_fails_before_unmet_done_work_can_dirty_the_repo() {
    let home = tempfile::tempdir().expect("create temporary home");
    let project = home.path().join("project");
    std::fs::create_dir_all(&project).expect("create project");
    init_project(&project);

    let lexical_target = project.join("target");
    let resolved_target = home.path().join("ssd").join("canonical-target");
    symlink(&resolved_target, &lexical_target).expect("create broken external target symlink");
    let fake_bin = install_editing_codex(&home.path().join("bin"));

    let mut command = csa_cmd(home.path());
    command
        .current_dir(&project)
        .env("PATH", prepend_path(&fake_bin))
        .args([
            "run",
            "--no-daemon",
            "--sa-mode",
            "true",
            "--tool",
            "codex",
            "--min-free-memory-mb",
            "0",
            "--no-post-exec-gate",
            "DONE WHEN: just test, build, and commit have a confirmed PASS.",
        ]);
    let output = output_with_timeout(&mut command, "CSA with broken canonical target");

    let combined = format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "blocked canonical target must return failure, not success: {combined}"
    );
    assert!(
        combined.contains("Cargo target preflight blocked before provider execution"),
        "failure must explain the pre-edit Cargo target preflight: {combined}"
    );
    assert!(combined.contains(&format!("lexical path '{}'", lexical_target.display())));
    assert!(combined.contains(&format!("resolves to '{}'", resolved_target.display())));
    assert!(
        !project.join("provider-ran.txt").exists() && !project.join("dirty-edit.txt").exists(),
        "the provider must not run or leave unmet-DONE edits behind: {combined}"
    );
    assert!(
        std::fs::symlink_metadata(&lexical_target)
            .expect("target symlink metadata")
            .file_type()
            .is_symlink(),
        "preflight must leave the configured symlink in place"
    );
}

#[test]
fn unconfirmed_zsh_gate_wrapper_is_terminal_failure_after_dirty_edit() {
    let home = tempfile::tempdir().expect("create temporary home");
    let project = home.path().join("project");
    std::fs::create_dir_all(&project).expect("create project");
    init_project(&project);

    let fake_bin = install_editing_codex(&home.path().join("bin"));
    let codex = fake_bin.join("codex");
    std::fs::write(
        &codex,
        r#"#!/bin/sh
printf 'provider reached\n' > provider-ran.txt
printf 'dirty edit\n' > dirty-edit.txt
printf '%s\n' '{"type":"thread.started","thread_id":"unconfirmed-zsh-gate"}' '{"type":"item.completed","item":{"type":"agent_message","text":"zsh: read-only variable: status"}}' '{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}'
"#,
    )
    .expect("write unconfirmed-gate fake codex");

    let mut command = csa_cmd(home.path());
    command
        .current_dir(&project)
        .env("PATH", prepend_path(&fake_bin))
        .args([
            "run",
            "--no-daemon",
            "--sa-mode",
            "true",
            "--tool",
            "codex",
            "--min-free-memory-mb",
            "0",
            "--no-post-exec-gate",
            "DONE WHEN: the gate has a confirmed PASS.",
        ]);
    let output = output_with_timeout(&mut command, "CSA with unconfirmed zsh gate");

    let combined = format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "an unconfirmed gate exit must be terminal failure, not success: {combined}"
    );
    assert!(
        project.join("dirty-edit.txt").exists(),
        "fake writer must have edited"
    );
}

#[test]
fn unmet_done_summary_is_terminal_failure_after_dirty_edit_without_blocked_marker() {
    let home = tempfile::tempdir().expect("create temporary home");
    let project = home.path().join("project");
    std::fs::create_dir_all(&project).expect("create project");
    init_project(&project);

    let fake_bin = install_editing_codex(&home.path().join("bin"));
    let codex = fake_bin.join("codex");
    std::fs::write(
        &codex,
        r#"#!/bin/sh
printf 'provider reached\n' > provider-ran.txt
printf 'dirty edit\n' > dirty-edit.txt
printf '%s\n' '{"type":"thread.started","thread_id":"unmet-done"}' '{"type":"item.completed","item":{"type":"agent_message","text":"tests and commit omitted"}}' '{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}'
"#,
    )
    .expect("write unmet-DONE fake codex");

    let mut command = csa_cmd(home.path());
    command
        .current_dir(&project)
        .env("PATH", prepend_path(&fake_bin))
        .args([
            "run",
            "--no-daemon",
            "--sa-mode",
            "true",
            "--tool",
            "codex",
            "--min-free-memory-mb",
            "0",
            "--no-post-exec-gate",
            "DONE WHEN: tests, build, and commit have a confirmed PASS.",
        ]);
    let output = output_with_timeout(&mut command, "CSA with unmet DONE summary");

    let combined = format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "dirty SA-mode work without a completion receipt must fail: {combined}"
    );
    assert!(
        project.join("dirty-edit.txt").exists(),
        "fake writer must have edited"
    );
}
