//! Daemon spawning: detach a child process with setsid + redirected I/O.
//!
//! Low-level utility. Does NOT know about CLI parsing, session
//! management, or CSA configuration.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

#[path = "daemon_cleanup.rs"]
mod cleanup;
#[cfg(test)]
use cleanup::stop_systemd_scope_with_timeout;
use cleanup::{
    SpawnedProcessCleanup, SpawnedProcessLiveness, inspect_spawned_process_without_reaping,
    terminate_and_reap_spawned_daemon,
};

const DAEMON_INDEPENDENT_SCOPE_ENV: &str = "CSA_DAEMON_INDEPENDENT_SCOPE";

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

#[derive(Debug, Clone, PartialEq, Eq)]
enum DaemonSpawnMode {
    Direct,
    IndependentScope { unit: String },
}

fn open_log_file(dir: &Path, name: &str) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(dir.join(name))
        .with_context(|| format!("failed to create {name} in {}", dir.display()))
}

fn open_log_file_append(dir: &Path, name: &str) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(dir.join(name))
        .with_context(|| format!("failed to open {name} for append in {}", dir.display()))
}

fn daemon_pid_record(pid: u32) -> String {
    match read_process_start_time_ticks(pid) {
        Some(start_time_ticks) => format!("{pid} {start_time_ticks}\n"),
        None => format!("{pid}\n"),
    }
}

fn daemon_spawn_mode(session_id: &str) -> DaemonSpawnMode {
    match std::env::var(DAEMON_INDEPENDENT_SCOPE_ENV).as_deref() {
        Ok("0" | "false" | "off" | "no") => return DaemonSpawnMode::Direct,
        Ok("1" | "true" | "on" | "yes") => {
            return DaemonSpawnMode::IndependentScope {
                unit: csa_resource::cgroup::scope_unit_name("daemon", session_id),
            };
        }
        _ => {}
    }

    if inherited_csa_scope_detected() {
        if csa_resource::sandbox::has_systemd_user_scope() {
            DaemonSpawnMode::IndependentScope {
                unit: csa_resource::cgroup::scope_unit_name("daemon", session_id),
            }
        } else {
            tracing::warn!(
                "inherited CSA systemd scope detected but `systemd-run --user --scope` probe \
                 failed (dbus likely unavailable in nested CSA subprocess); \
                 daemon will spawn as direct detached process"
            );
            DaemonSpawnMode::Direct
        }
    } else {
        DaemonSpawnMode::Direct
    }
}

fn inherited_csa_scope_detected() -> bool {
    std::fs::read_to_string("/proc/self/cgroup").is_ok_and(|content| {
        content
            .lines()
            .any(|line| line.contains("csa-") && line.contains(".scope"))
    })
}

fn append_daemon_child_args(cmd: &mut Command, config: &DaemonSpawnConfig) {
    for verb in config.subcommand.split_whitespace() {
        cmd.arg(verb);
    }
    cmd.args(["--daemon-child", "--session-id", &config.session_id]);
    cmd.args(&config.args);

    for (k, v) in &config.env {
        cmd.env(k, v);
    }
}

fn build_daemon_command(config: &DaemonSpawnConfig, mode: &DaemonSpawnMode) -> Command {
    build_daemon_command_with_systemd_run(config, mode, Path::new("systemd-run"))
}

fn build_daemon_command_with_systemd_run(
    config: &DaemonSpawnConfig,
    mode: &DaemonSpawnMode,
    systemd_run: &Path,
) -> Command {
    match mode {
        DaemonSpawnMode::Direct => {
            let mut cmd = Command::new(&config.csa_binary);
            append_daemon_child_args(&mut cmd, config);
            cmd
        }
        DaemonSpawnMode::IndependentScope { unit } => {
            let mut cmd = Command::new(systemd_run);
            cmd.args(["--user", "--scope", "--quiet", "--collect", "--unit", unit]);
            cmd.arg("--");
            cmd.arg(&config.csa_binary);
            append_daemon_child_args(&mut cmd, config);
            cmd
        }
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
    spawn_daemon_with_systemd_run(config, Path::new("systemd-run"))
}

/// Spawn a daemon, run a caller-provided readiness check while the child is
/// still owned by this process, and detach only after the check succeeds.
///
/// A failed check stops an independent scope when present, terminates the
/// anchored spawned process group, reaps its leader, and removes matching
/// PID/scope records after successful cleanup. If a liveness probe has already
/// reaped the leader, cleanup never signals its stale PGID; it may still stop
/// the exact recorded systemd unit. Spool logs remain for diagnostics.
pub fn spawn_daemon_verified<F>(config: DaemonSpawnConfig, verify: F) -> Result<DaemonSpawnResult>
where
    F: FnOnce(&DaemonSpawnResult) -> Result<()>,
{
    spawn_daemon_verified_and_publish(config, verify, |_, ()| Ok(()))
}

/// Spawn and verify a daemon, then publish its caller-visible start marker
/// while the child is still owned. A verification, liveness, or publication
/// failure triggers bounded cleanup before the error is returned.
pub fn spawn_daemon_verified_and_publish<T, F, P>(
    config: DaemonSpawnConfig,
    verify: F,
    publish: P,
) -> Result<DaemonSpawnResult>
where
    F: FnOnce(&DaemonSpawnResult) -> Result<T>,
    P: FnOnce(&DaemonSpawnResult, T) -> Result<()>,
{
    spawn_daemon_verified_with_commands_and_publish(
        config,
        Path::new("systemd-run"),
        Path::new("systemctl"),
        verify,
        publish,
    )
}

fn spawn_daemon_with_systemd_run(
    config: DaemonSpawnConfig,
    systemd_run: &Path,
) -> Result<DaemonSpawnResult> {
    spawn_daemon_verified_with_systemd_run(config, systemd_run, |_| Ok(()))
}

fn spawn_daemon_verified_with_systemd_run<F>(
    config: DaemonSpawnConfig,
    systemd_run: &Path,
    verify: F,
) -> Result<DaemonSpawnResult>
where
    F: FnOnce(&DaemonSpawnResult) -> Result<()>,
{
    spawn_daemon_verified_with_commands(config, systemd_run, Path::new("systemctl"), verify)
}

fn spawn_daemon_verified_with_commands<F>(
    config: DaemonSpawnConfig,
    systemd_run: &Path,
    systemctl: &Path,
    verify: F,
) -> Result<DaemonSpawnResult>
where
    F: FnOnce(&DaemonSpawnResult) -> Result<()>,
{
    spawn_daemon_verified_with_commands_and_publish(
        config,
        systemd_run,
        systemctl,
        verify,
        |_, ()| Ok(()),
    )
}

fn spawn_daemon_verified_with_commands_and_publish<T, F, P>(
    config: DaemonSpawnConfig,
    systemd_run: &Path,
    systemctl: &Path,
    verify: F,

    publish: P,
) -> Result<DaemonSpawnResult>
where
    F: FnOnce(&DaemonSpawnResult) -> Result<T>,
    P: FnOnce(&DaemonSpawnResult, T) -> Result<()>,
{
    std::fs::create_dir_all(&config.session_dir).with_context(|| {
        format!(
            "failed to create session dir {}",
            config.session_dir.display()
        )
    })?;

    let stdout_file = open_log_file(&config.session_dir, "stdout.log")?;
    let mut stderr_file = open_log_file(&config.session_dir, "stderr.log")?;

    let spawn_mode = daemon_spawn_mode(&config.session_id);
    match &spawn_mode {
        DaemonSpawnMode::Direct => {
            remove_file_if_exists(&config.session_dir.join("daemon.scope"))?;
            writeln!(
                stderr_file,
                "CSA daemon spawn: direct detached process; no inherited CSA systemd scope detected"
            )?;
        }
        DaemonSpawnMode::IndependentScope { unit } => {
            std::fs::write(config.session_dir.join("daemon.scope"), unit)?;
            writeln!(
                stderr_file,
                "CSA daemon spawn: independent systemd scope {unit}; inherited CSA scope detected"
            )?;
        }
    }

    let mut cmd = build_daemon_command_with_systemd_run(&config, &spawn_mode, systemd_run);

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

    let (mut child, effective_spawn_mode) = match cmd.spawn() {
        Ok(child) => (child, spawn_mode),
        Err(scope_err) if matches!(spawn_mode, DaemonSpawnMode::IndependentScope { .. }) => {
            // systemd-run binary not found or exec failed; fall back to direct detach.
            tracing::warn!(
                error = %scope_err,
                "daemon spawn via systemd scope failed; retrying as direct detached process"
            );
            let stdout2 = open_log_file_append(&config.session_dir, "stdout.log")?;
            let mut stderr2 = open_log_file_append(&config.session_dir, "stderr.log")?;
            writeln!(
                stderr2,
                "CSA daemon spawn: scope spawn failed ({scope_err}), retrying as direct detached process"
            )?;
            remove_file_if_exists(&config.session_dir.join("daemon.scope"))?;
            let mut cmd2 = build_daemon_command(&config, &DaemonSpawnMode::Direct);
            cmd2.stdin(Stdio::null());
            cmd2.stdout(stdout2);
            cmd2.stderr(stderr2);
            // SAFETY: same as above.
            unsafe {
                cmd2.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            (
                cmd2.spawn()
                    .context("daemon spawn retry (direct mode) also failed")?,
                DaemonSpawnMode::Direct,
            )
        }
        Err(e) => return Err(e).context("failed to spawn daemon child process"),
    };

    let pid = child.id();

    let result = DaemonSpawnResult {
        pid,
        session_id: config.session_id,
        session_dir: config.session_dir,
    };

    // Write daemon PID file for `csa session kill` and `wait` liveness checks.
    let pid_path = result.session_dir.join("daemon.pid");
    if let Err(error) = std::fs::write(&pid_path, daemon_pid_record(pid))
        .with_context(|| format!("failed to write {}", pid_path.display()))
    {
        return Err(cleanup_after_spawn_error(
            error,
            "daemon PID record setup failed",
            SpawnedProcessCleanup::ProcessGroupAnchor(&mut child),
            &effective_spawn_mode,
            systemctl,
            &result,
        ));
    }

    let verified = match verify(&result) {
        Ok(verified) => verified,
        Err(error) => {
            return Err(cleanup_after_spawn_error(
                error,
                "daemon readiness verification failed",
                SpawnedProcessCleanup::ProcessGroupAnchor(&mut child),
                &effective_spawn_mode,
                systemctl,
                &result,
            ));
        }
    };

    // Detach only after all caller-supplied pre-marker checks pass. On Linux,
    // inspect with waitid(WNOWAIT) so an exited leader remains the owned PGID
    // anchor until failure cleanup terminates every descendant.
    let child_liveness = match inspect_spawned_process_without_reaping(&mut child) {
        Ok(status) => status,
        Err(error) => {
            return Err(cleanup_after_spawn_error(
                error,
                "spawned-daemon liveness check failed",
                SpawnedProcessCleanup::WaitStateUnknown,
                &effective_spawn_mode,
                systemctl,
                &result,
            ));
        }
    };
    match child_liveness {
        SpawnedProcessLiveness::Running => {}
        SpawnedProcessLiveness::Exited(status) => {
            let exited_err = anyhow::anyhow!(
                "daemon process {pid} exited before readiness verification completed: {status}"
            );
            return Err(cleanup_after_spawn_error(
                exited_err,
                "spawned daemon exited early",
                SpawnedProcessCleanup::ProcessGroupAnchor(&mut child),
                &effective_spawn_mode,
                systemctl,
                &result,
            ));
        }
        #[cfg(not(target_os = "linux"))]
        SpawnedProcessLiveness::AlreadyReaped(status) => {
            let exited_err = anyhow::anyhow!(
                "daemon process {pid} exited before readiness verification completed: {status}"
            );
            return Err(cleanup_after_spawn_error(
                exited_err,
                "spawned daemon exited early",
                SpawnedProcessCleanup::AlreadyReaped,
                &effective_spawn_mode,
                systemctl,
                &result,
            ));
        }
    }

    if let Err(error) = publish(&result, verified) {
        return Err(cleanup_after_spawn_error(
            error,
            "daemon start-marker publication failed",
            SpawnedProcessCleanup::ProcessGroupAnchor(&mut child),
            &effective_spawn_mode,
            systemctl,
            &result,
        ));
    }

    // Release the handle without waiting. std::process::Child has no
    // kill-on-drop behavior, so the successfully verified daemon stays detached.
    drop(child);
    Ok(result)
}

fn cleanup_after_spawn_error(
    primary: anyhow::Error,
    context: &str,
    process: SpawnedProcessCleanup<'_>,
    spawn_mode: &DaemonSpawnMode,
    systemctl: &Path,
    result: &DaemonSpawnResult,
) -> anyhow::Error {
    let primary = primary.context(context.to_string());
    match terminate_and_reap_spawned_daemon(process, spawn_mode, systemctl)
        .and_then(|()| remove_spawn_lifecycle_records(result, spawn_mode))
    {
        Ok(()) => primary,
        Err(cleanup_error) => primary.context(format!(
            "spawned-daemon cleanup also failed: {cleanup_error:#}"
        )),
    }
}

fn remove_spawn_lifecycle_records(
    result: &DaemonSpawnResult,
    spawn_mode: &DaemonSpawnMode,
) -> Result<()> {
    let pid_path = result.session_dir.join("daemon.pid");
    match std::fs::read_to_string(&pid_path) {
        Ok(contents) => {
            let recorded_pid = contents
                .split_whitespace()
                .next()
                .and_then(|v| v.parse().ok());
            if recorded_pid == Some(result.pid) {
                remove_file_if_exists(&pid_path)?;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context(format!("failed to read {}", pid_path.display())),
    }

    if let DaemonSpawnMode::IndependentScope { unit } = spawn_mode {
        let scope_path = result.session_dir.join("daemon.scope");
        match std::fs::read_to_string(&scope_path) {
            Ok(contents) if contents.trim() == unit => remove_file_if_exists(&scope_path)?,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).context(format!("failed to read {}", scope_path.display()));
            }
        }
    }
    Ok(())
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context(format!("failed to remove {}", path.display())),
    }
}

#[cfg(test)]
static DAEMON_SCOPE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
struct DaemonScopeEnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for DaemonScopeEnvGuard {
    fn drop(&mut self) {
        // SAFETY: daemon spawn tests serialize environment mutation through
        // DAEMON_SCOPE_ENV_LOCK and restore the variable before releasing it.
        unsafe {
            std::env::remove_var(DAEMON_INDEPENDENT_SCOPE_ENV);
        }
    }
}

#[cfg(test)]
fn force_direct_daemon_spawn_for_test() -> DaemonScopeEnvGuard {
    let lock = DAEMON_SCOPE_ENV_LOCK
        .lock()
        .expect("daemon env lock poisoned");
    // SAFETY: daemon spawn tests serialize environment mutation through the
    // shared lock and restore the variable in DaemonScopeEnvGuard.
    unsafe {
        std::env::set_var(DAEMON_INDEPENDENT_SCOPE_ENV, "0");
    }
    DaemonScopeEnvGuard { _lock: lock }
}

#[cfg(test)]
fn force_independent_scope_for_test() -> DaemonScopeEnvGuard {
    let lock = DAEMON_SCOPE_ENV_LOCK
        .lock()
        .expect("daemon env lock poisoned");
    // SAFETY: same serialized test-only environment mutation as above.
    unsafe {
        std::env::set_var(DAEMON_INDEPENDENT_SCOPE_ENV, "1");
    }
    DaemonScopeEnvGuard { _lock: lock }
}

#[cfg(test)]
#[path = "daemon_lifecycle_tests.rs"]
mod lifecycle_tests;

#[cfg(test)]
#[path = "daemon_tests.rs"]
mod tests;
