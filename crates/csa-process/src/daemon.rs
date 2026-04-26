//! Daemon spawning: detach a child process with setsid + redirected I/O.
//!
//! Low-level utility. Does NOT know about CLI parsing, session
//! management, or CSA configuration.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

/// Configuration for spawning a daemonized child process.
pub struct DaemonSpawnConfig {
    pub session_id: String,
    pub session_dir: PathBuf,
    pub csa_binary: PathBuf,
    /// Subcommand path for the child process. May be a single verb
    /// (e.g. "run", "review", "debate") or a space-separated nested path
    /// (e.g. "plan run"). Splits on whitespace at exec time.
    pub subcommand: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

/// Result of a successful daemon spawn.
pub struct DaemonSpawnResult {
    pub pid: u32,
    pub session_id: String,
    pub session_dir: PathBuf,
}

fn open_log_file(dir: &std::path::Path, name: &str) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(dir.join(name))
        .with_context(|| format!("failed to create {name} in {}", dir.display()))
}

fn daemon_pid_record(pid: u32) -> String {
    match read_process_start_time_ticks(pid) {
        Some(start_time_ticks) => format!("{pid} {start_time_ticks}\n"),
        None => format!("{pid}\n"),
    }
}

#[cfg(unix)]
fn read_process_start_time_ticks(pid: u32) -> Option<u64> {
    let stat_path = format!("/proc/{pid}/stat");
    let content = std::fs::read_to_string(stat_path).ok()?;
    let close_paren = content.rfind(')')?;
    let after_comm = &content[close_paren + 1..];
    let mut parts = after_comm.split_whitespace();
    parts.next()?;
    parts.next()?;
    parts.next()?;
    for _ in 0..16 {
        parts.next()?;
    }
    parts.next()?.parse::<u64>().ok()
}

#[cfg(not(unix))]
fn read_process_start_time_ticks(_pid: u32) -> Option<u64> {
    None
}

/// Spawn a detached daemon process with setsid, stdin=/dev/null,
/// stdout/stderr redirected to spool files in the session directory.
pub fn spawn_daemon(config: DaemonSpawnConfig) -> Result<DaemonSpawnResult> {
    std::fs::create_dir_all(&config.session_dir).with_context(|| {
        format!(
            "failed to create session dir {}",
            config.session_dir.display()
        )
    })?;

    let stdout_file = open_log_file(&config.session_dir, "stdout.log")?;
    let stderr_file = open_log_file(&config.session_dir, "stderr.log")?;

    let mut cmd = Command::new(&config.csa_binary);
    for verb in config.subcommand.split_whitespace() {
        cmd.arg(verb);
    }
    cmd.args(["--daemon-child", "--session-id", &config.session_id]);
    cmd.args(&config.args);

    for (k, v) in &config.env {
        cmd.env(k, v);
    }

    cmd.stdin(Stdio::null());
    cmd.stdout(stdout_file);
    cmd.stderr(stderr_file);

    // SAFETY: setsid() is async-signal-safe (POSIX), called between
    // fork and exec to detach from parent session/process group.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = cmd
        .spawn()
        .context("failed to spawn daemon child process")?;

    let pid = child.id();

    // Write daemon PID file for `csa session kill` and `wait` liveness checks.
    let pid_path = config.session_dir.join("daemon.pid");
    std::fs::write(&pid_path, daemon_pid_record(pid))
        .with_context(|| format!("failed to write {}", pid_path.display()))?;

    // Detach: the daemon child will outlive us. We must not leave a
    // zombie, so `try_wait` reaps it if it already exited (unlikely)
    // and `forget` prevents the Drop impl from killing the child.
    let _ = child.try_wait();
    // Intentionally leak the Child handle so Drop doesn't kill the daemon.
    std::mem::forget(child);

    Ok(DaemonSpawnResult {
        pid,
        session_id: config.session_id,
        session_dir: config.session_dir,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// Write a wrapper that LOGS every received arg on its own line, then
    /// skips daemon-child prefix args until `--` and evals the rest.
    ///
    /// The per-arg `arg=<token>` log is what
    /// `test_daemon_spawn_supports_multi_word_subcommand` inspects to prove
    /// that the multi-word subcommand was actually split into distinct
    /// argv tokens (`plan` and `run`), not passed as a single
    /// `"plan run"` token. Without this, the wrapper's pre-`--` consume
    /// loop would discard the evidence and the assertion would pass
    /// vacuously.
    fn write_wrapper_script(dir: &std::path::Path, name: &str) -> PathBuf {
        use std::io::Write;
        let script = dir.join(name);
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o755)
            .open(&script)
            .expect("create wrapper script");
        f.write_all(
            b"#!/bin/sh\n\
              # Log every received arg first (one per line) so tests can\n\
              # assert on how the spawner split the subcommand path.\n\
              for tok in \"$@\"; do\n  echo \"arg=$tok\"\ndone\n\
              # Then skip all args until '--' and eval the rest.\n\
              while [ \"$#\" -gt 0 ]; do\n  case \"$1\" in --) shift; break;; *) shift;; esac\ndone\n\
              eval \"$@\"\n",
        )
        .expect("write wrapper script");
        f.sync_all().expect("sync wrapper script");
        drop(f);
        script
    }

    #[test]
    fn test_daemon_spawn_creates_spool_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let session_dir = tmp.path().join("session-test");
        let wrapper = write_wrapper_script(tmp.path(), "wrapper1.sh");

        let config = DaemonSpawnConfig {
            session_id: "TEST001".to_string(),
            session_dir: session_dir.clone(),
            csa_binary: wrapper,
            subcommand: "run".to_string(),
            // After the injected flags, pass '--' then the real command.
            args: vec!["--".to_string(), "echo hello".to_string()],
            env: HashMap::new(),
        };

        let result = spawn_daemon(config).expect("spawn_daemon");
        assert_eq!(result.session_id, "TEST001");
        assert!(result.pid > 0);

        // Give the child time to write and exit.
        std::thread::sleep(std::time::Duration::from_millis(500));

        let stdout_path = session_dir.join("stdout.log");
        let stderr_path = session_dir.join("stderr.log");
        assert!(stdout_path.exists(), "stdout.log must exist");
        assert!(stderr_path.exists(), "stderr.log must exist");

        let mut contents = String::new();
        File::open(&stdout_path)
            .expect("open stdout.log")
            .read_to_string(&mut contents)
            .expect("read stdout.log");
        assert!(
            contents.contains("hello"),
            "stdout.log should contain 'hello', got: {contents:?}"
        );
    }

    #[test]
    fn test_daemon_spawn_supports_multi_word_subcommand() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let session_dir = tmp.path().join("session-multi");
        let wrapper = write_wrapper_script(tmp.path(), "wrapper-multi.sh");

        let config = DaemonSpawnConfig {
            session_id: "TEST_MULTI".to_string(),
            session_dir: session_dir.clone(),
            csa_binary: wrapper,
            subcommand: "plan run".to_string(),
            args: vec!["--".to_string(), "echo got=$1,$2,$3,$4,$5".to_string()],
            env: HashMap::new(),
        };

        let result = spawn_daemon(config).expect("spawn_daemon");
        assert!(result.pid > 0);
        std::thread::sleep(std::time::Duration::from_millis(500));

        let mut contents = String::new();
        File::open(session_dir.join("stdout.log"))
            .expect("open stdout.log")
            .read_to_string(&mut contents)
            .expect("read stdout.log");

        // The wrapper logs every received arg as `arg=<token>` on its own
        // line BEFORE consuming up to `--`. The actual exec was:
        //   <wrapper> plan run --daemon-child --session-id TEST_MULTI -- echo got=...
        // Assert on the split: `plan` and `run` MUST appear on DISTINCT
        // arg= lines. Without distinct lines, `subcommand: "plan run"`
        // could have been passed as a single argv token and we wouldn't
        // notice — that was the original test's vacuous-pass bug
        // (#1130 PR-1 review F2).
        let arg_lines: Vec<&str> = contents.lines().filter(|l| l.starts_with("arg=")).collect();
        assert!(
            arg_lines.iter().any(|l| *l == "arg=plan"),
            "expected a distinct `arg=plan` line proving the subcommand was \
             split, got arg lines: {arg_lines:?}"
        );
        assert!(
            arg_lines.iter().any(|l| *l == "arg=run"),
            "expected a distinct `arg=run` line proving the subcommand was \
             split, got arg lines: {arg_lines:?}"
        );
        // Sanity: the daemon-child prefix the spawner injects must also be
        // present so we know we're inspecting the real exec, not a noop.
        assert!(
            arg_lines.iter().any(|l| *l == "arg=--daemon-child"),
            "expected `arg=--daemon-child` from the spawner injection, got \
             arg lines: {arg_lines:?}"
        );
        assert!(
            contents.contains("got="),
            "stdout should still contain 'got=' (exec ran), got: {contents:?}"
        );
    }

    #[test]
    fn test_daemon_spawn_child_detached() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let session_dir = tmp.path().join("session-detach");
        let wrapper = write_wrapper_script(tmp.path(), "wrapper2.sh");

        let config = DaemonSpawnConfig {
            session_id: "TEST002".to_string(),
            session_dir: session_dir.clone(),
            csa_binary: wrapper,
            subcommand: "run".to_string(),
            args: vec![
                "--".to_string(),
                "echo pid=$$ sid=$(ps -o sid= -p $$)".to_string(),
            ],
            env: HashMap::new(),
        };

        let result = spawn_daemon(config).expect("spawn_daemon");
        let child_pid = result.pid;
        let parent_pid = std::process::id();

        assert_ne!(child_pid, parent_pid, "child PID must differ from parent");

        // Give the child time to write and exit.
        std::thread::sleep(std::time::Duration::from_millis(500));

        let mut contents = String::new();
        File::open(session_dir.join("stdout.log"))
            .expect("open stdout.log")
            .read_to_string(&mut contents)
            .expect("read stdout.log");

        // Parse the sid= value from output and verify it differs from
        // the parent's session ID.
        if let Some(sid_str) = contents.split("sid=").nth(1) {
            let child_sid: u32 = sid_str.trim().parse().unwrap_or(0);
            // SAFETY: libc::getsid is safe for the current process.
            let parent_sid = unsafe { libc::getsid(0) } as u32;
            assert_ne!(
                child_sid, parent_sid,
                "child session ID ({child_sid}) must differ from parent ({parent_sid})"
            );
        }
    }
}
