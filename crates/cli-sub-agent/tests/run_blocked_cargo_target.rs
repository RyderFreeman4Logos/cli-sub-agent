#![cfg(target_os = "linux")]

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
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

struct ProcessIdentity {
    pid: u32,
    start_time: u64,
    #[cfg(target_os = "linux")]
    pidfd: Option<OwnedFd>,
}

impl ProcessIdentity {
    fn capture(pid: u32) -> Option<Self> {
        let (_, start_time) = process_state_and_start_time(pid)?;
        Self::capture_with_start_time(pid, start_time)
    }

    fn capture_with_start_time(pid: u32, expected_start_time: u64) -> Option<Self> {
        let (_, start_time) = process_state_and_start_time(pid)?;
        if start_time != expected_start_time {
            return None;
        }
        Some(Self {
            pid,
            start_time,
            #[cfg(target_os = "linux")]
            pidfd: open_pidfd(pid),
        })
    }

    fn still_matches(&self) -> bool {
        process_state_and_start_time(self.pid)
            .is_some_and(|(_, start_time)| start_time == self.start_time)
    }

    fn signal_direct_if_current(&self, signal: libc::c_int) -> bool {
        if !self.still_matches() {
            return false;
        }
        #[cfg(target_os = "linux")]
        if let Some(pidfd) = &self.pidfd {
            // SAFETY: `pidfd` is bound to this verified process identity, so
            // pidfd_send_signal cannot target a reused numeric PID.
            return unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    pidfd.as_raw_fd(),
                    signal,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                ) == 0
            };
        }
        // SAFETY: start time was verified immediately before this fallback
        // signal, and the helper never signals an identity it has reaped.
        unsafe { libc::kill(self.pid as i32, signal) == 0 }
    }
}

#[cfg(target_os = "linux")]
fn open_pidfd(pid: u32) -> Option<OwnedFd> {
    // SAFETY: pidfd_open receives a valid pid and flags=0; its successful file
    // descriptor is immediately owned by `OwnedFd` and closed on drop.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) };
    (fd >= 0).then(|| {
        // SAFETY: this descriptor was returned by pidfd_open above.
        unsafe { OwnedFd::from_raw_fd(fd as i32) }
    })
}

struct ChildProcessGroupGuard {
    child: std::process::Child,
    leader: ProcessIdentity,
    reaped: bool,
}

impl ChildProcessGroupGuard {
    fn new(child: std::process::Child) -> Self {
        let leader = ProcessIdentity::capture(child.id())
            .expect("capture direct child process identity before cleanup");
        Self {
            child,
            leader,
            reaped: false,
        }
    }

    fn exited_without_reaping(&self) -> std::io::Result<bool> {
        // SAFETY: all-zero bytes are a valid initialized baseline for the C
        // siginfo_t output buffer before waitid writes its observed status.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is writable initialized storage; the direct child is
        // still unreaped, and WNOWAIT observes it without losing group authority.
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                self.leader.pid,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: successful waitid initialized `info`; si_pid is zero for a
        // WNOHANG miss and is the observed child PID otherwise.
        Ok(unsafe { info.si_pid() } == self.leader.pid as libc::pid_t)
    }

    fn fence_group_if_owned(&self) -> bool {
        if !self.leader.still_matches() {
            return false;
        }
        // SAFETY: process_group(0) makes the direct child the group leader. The
        // guard retains that child unreaped and verifies its start time before
        // signalling, preventing this negative PGID from being reused first.
        unsafe { libc::kill(-(self.leader.pid as i32), libc::SIGKILL) == 0 }
    }

    fn reap(&mut self) -> std::io::Result<std::process::ExitStatus> {
        let status = self.child.wait();
        self.reaped = true;
        status
    }

    fn cleanup(&mut self) {
        if !self.reaped {
            let _ = self.fence_group_if_owned();
            let _ = self.reap();
        }
    }
}

impl Drop for ChildProcessGroupGuard {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn process_state_and_start_time(pid: u32) -> Option<(char, u64)> {
    let stat_path = format!("/proc/{pid}/stat");
    let stat = std::fs::read_to_string(stat_path).ok()?;
    let close_paren = stat.rfind(')')?;
    let mut fields = stat.get(close_paren + 2..)?.split_whitespace();
    let state = fields.next()?.chars().next()?;
    let start_time = fields.nth(18)?.parse().ok()?;
    Some((state, start_time))
}

fn process_is_running(pid: u32) -> bool {
    // A killed-but-not-yet-reaped process is not a surviving workload.
    process_state_and_start_time(pid).is_some_and(|(state, _)| state != 'Z')
}

fn wait_for_process_exit(pid: u32) -> bool {
    for _ in 0..100 {
        if !process_is_running(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn output_with_timeout(command: &mut Command, description: &str) -> Output {
    output_with_deadline(command, description, COMMAND_TIMEOUT)
}

fn output_with_deadline(
    command: &mut Command,
    description: &str,
    command_timeout: Duration,
) -> Output {
    output_with_deadline_observing(command, description, command_timeout, || String::new())
}

fn output_with_deadline_observing<F>(
    command: &mut Command,
    description: &str,
    command_timeout: Duration,
    mut observe_before_cleanup: F,
) -> Output
where
    F: FnMut() -> String,
{
    command.process_group(0);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("spawn {description}: {error}"));
    let stdout = drain_pipe(child.stdout.take().expect("piped stdout"));
    let stderr = drain_pipe(child.stderr.take().expect("piped stderr"));
    let mut child = ChildProcessGroupGuard::new(child);
    let deadline = Instant::now() + command_timeout;
    let status = loop {
        let observation = observe_before_cleanup();
        match child.exited_without_reaping() {
            Ok(true) => {
                let _ = child.fence_group_if_owned();
                break child
                    .reap()
                    .unwrap_or_else(|error| panic!("reap {description}: {error}"));
            }
            Ok(false) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(false) => {
                child.cleanup();
                panic!(
                    "{description} did not finish within {} seconds; {observation}",
                    command_timeout.as_secs(),
                );
            }
            Err(error) => {
                child.cleanup();
                panic!("inspect {description}: {error}");
            }
        }
    };

    let Some(stdout) = receive_pipe_before_deadline(&stdout, deadline, description) else {
        panic!(
            "{description} did not drain stdout within {} seconds",
            command_timeout.as_secs()
        );
    };
    let Some(stderr) = receive_pipe_before_deadline(&stderr, deadline, description) else {
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
fn output_reaps_descendants_after_direct_child_exits_and_pipes_drain() {
    let identity_dir = tempfile::tempdir().expect("create descendant identity directory");
    let identity_path = identity_dir.path().join("descendant.identity");
    let release_path = identity_dir.path().join("release");
    let mut command = Command::new("sh");
    let script = format!(
        "sleep 300 </dev/null >/dev/null 2>&1 & descendant=$!; start_time=$(awk '{{print $22}}' \"/proc/$descendant/stat\"); printf '%s %s\\n' \"$descendant\" \"$start_time\" > \"{}\"; printf '%s %s\\n' \"$descendant\" \"$start_time\"; while [ ! -e \"{}\" ]; do sleep 0.01; done; exit 0",
        identity_path.display(),
        release_path.display(),
    );
    command.args(["-c", &script]);
    let started = Instant::now();
    let mut descendant = None;
    let output = output_with_deadline_observing(
        &mut command,
        "direct child exited but descendant survived with closed pipes",
        Duration::from_secs(5),
        || {
            if descendant.is_some() {
                return String::from("captured descendant identity");
            }
            let Ok(identity_output) = std::fs::read_to_string(&identity_path) else {
                return String::from("descendant identity has not been published");
            };
            let identity = identity_output.split_whitespace().collect::<Vec<_>>();
            if identity.len() != 2 {
                return format!("invalid descendant identity contents: {identity_output:?}");
            }
            let Ok(pid) = identity[0].parse::<u32>() else {
                return format!("invalid descendant PID: {:?}", identity[0]);
            };
            let Ok(start_time) = identity[1].parse::<u64>() else {
                return format!("invalid descendant start time: {:?}", identity[1]);
            };
            if let Some(captured) = ProcessIdentity::capture_with_start_time(pid, start_time) {
                descendant = Some(captured);
                std::fs::write(&release_path, "released").expect("release direct child");
                return format!("captured descendant identity pid={pid} start_time={start_time}");
            }
            format!(
                "published descendant identity pid={pid} start_time={start_time} no longer matches"
            )
        },
    );
    let descendant =
        descendant.expect("capture the spawned descendant identity before checking cleanup");
    let exited = wait_for_process_exit(descendant.pid);
    if !exited {
        // Keep the regression hygienic if cleanup regresses: signal only the
        // descendant identity captured before the helper checked process exit.
        assert!(
            descendant.signal_direct_if_current(libc::SIGKILL),
            "cleanup must signal only the captured descendant identity"
        );
        let _ = wait_for_process_exit(descendant.pid);
    }

    assert!(
        output.status.success(),
        "direct child should exit successfully"
    );
    assert!(exited, "owned descendant must not survive a normal return");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "normal direct-child completion must remain bounded"
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
        // Keep the deliberate 9000MB writer contract hermetic under full-suite
        // host pressure. Debug-only hook; release builds keep production
        // admission intact (see pipeline_session_exec_pre_exec).
        .env("CSA_TEST_SKIP_HOST_MEMORY_ADMISSION", "1")
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
set -eu
printf 'provider reached\n' > "$CSA_PROJECT_ROOT/provider-ran.txt"
printf 'dirty edit\n' > "$CSA_PROJECT_ROOT/dirty-edit.txt"
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
set -eu
printf 'provider reached\n' > "$CSA_PROJECT_ROOT/provider-ran.txt"
printf 'dirty edit\n' > "$CSA_PROJECT_ROOT/dirty-edit.txt"
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
set -eu
printf 'provider reached\n' > "$CSA_PROJECT_ROOT/provider-ran.txt"
printf 'dirty edit\n' > "$CSA_PROJECT_ROOT/dirty-edit.txt"
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
