//! Daemon spawn logic for execution commands (daemon mode is the default).
//! Shared by `csa run`, `csa review`, and `csa debate`.

use std::io::Write;

use anyhow::Result;

/// Guard returned by [`check_daemon_flags`] when running as daemon child.
///
/// Holds the stderr rotation guard (if installed) so that stderr.log rotation
/// remains active for the entire daemon child lifetime.  Must be kept alive
/// until the process exits.
///
/// Call [`finalize`](Self::finalize) explicitly before any `process::exit()`
/// since `exit()` skips Drop destructors.
pub(crate) struct DaemonChildGuard {
    /// Kept alive to maintain stderr rotation; dropped on process exit.
    _stderr_rotation: Option<csa_process::daemon_stderr::StderrRotationGuard>,
}

impl DaemonChildGuard {
    /// Explicitly shut down the stderr rotation guard before process exit.
    ///
    /// This is a no-op if no stderr rotation was installed.
    #[allow(dead_code)]
    pub(crate) fn finalize(&mut self) {
        if let Some(guard) = &mut self._stderr_rotation {
            guard.finalize();
        }
    }
}

/// Check daemon flags and either spawn+exit or propagate session ID.
///
/// Returns `Ok(guard)` when the caller should proceed with the child path.
/// The returned guard must be kept alive for the duration of the process.
/// **Never returns** when daemon spawn succeeds (calls `process::exit(0)`).
pub(crate) fn check_daemon_flags(
    subcommand: &str,
    no_daemon: bool,
    daemon_child: bool,
    session_id: &Option<String>,
    cd: Option<&str>,
) -> Result<DaemonChildGuard> {
    if !no_daemon && !daemon_child {
        if session_id.is_some() {
            anyhow::bail!("--session-id is an internal flag and must not be used directly");
        }
        spawn_and_exit(subcommand, cd)?;
    }
    let mut stderr_rotation = None;
    if let Some(sid) = session_id {
        // SAFETY: Runs in the daemon child before tokio spawns worker threads.
        unsafe { std::env::set_var("CSA_DAEMON_SESSION_ID", sid) };
        crate::session_cmds_daemon::seed_daemon_session_env(sid, cd);

        // Install stderr rotation so daemon stderr.log is bounded.
        stderr_rotation = install_daemon_stderr_rotation(sid, cd);
    }
    Ok(DaemonChildGuard {
        _stderr_rotation: stderr_rotation,
    })
}

/// Best-effort stderr rotation install for daemon child processes.
///
/// Reads `stderr_spool_max_mb` and `spool_keep_rotated` from project config.
/// Falls back to defaults on any error.
fn install_daemon_stderr_rotation(
    session_id: &str,
    cd: Option<&str>,
) -> Option<csa_process::daemon_stderr::StderrRotationGuard> {
    let project_root = crate::pipeline::determine_project_root(cd).ok()?;
    let session_dir = csa_session::get_session_dir(&project_root, session_id).ok()?;
    let stderr_path = session_dir.join("stderr.log");

    // Resolve config for spool limits (best effort — fall back to defaults).
    let (max_bytes, keep_rotated) = resolve_stderr_spool_config(&project_root);

    match csa_process::daemon_stderr::install_stderr_rotation(&stderr_path, max_bytes, keep_rotated)
    {
        Ok(guard) => Some(guard),
        Err(e) => {
            // Best effort: log to the original stderr.log before fd replacement.
            eprintln!("[csa] failed to install stderr rotation: {e}");
            None
        }
    }
}

fn resolve_stderr_spool_config(project_root: &std::path::Path) -> (u64, bool) {
    let project_cfg = csa_config::ProjectConfig::load(project_root).ok().flatten();

    if let Some(cfg) = project_cfg {
        let max_mb = cfg.session.resolved_stderr_spool_max_mb();
        let max_bytes = u64::from(max_mb).saturating_mul(1024 * 1024);
        let keep = cfg.session.resolved_spool_keep_rotated();
        return (max_bytes, keep);
    }

    (
        csa_process::daemon_stderr::DEFAULT_STDERR_SPOOL_MAX_BYTES,
        csa_process::DEFAULT_SPOOL_KEEP_ROTATED,
    )
}

/// Fork a daemon child and exit the parent process.
///
/// The daemon child will re-exec with `--daemon-child --session-id <ID>`
/// and the same flags the parent received. stdout.log / stderr.log are
/// captured in the session directory.
///
/// This function **never returns on success** — it calls `process::exit(0)`.
pub(crate) fn spawn_and_exit(subcommand: &str, cd: Option<&str>) -> Result<()> {
    let sid = csa_session::new_session_id();
    let project_root = crate::pipeline::determine_project_root(cd)?;
    let session_root = csa_session::get_session_root(&project_root)?;
    let session_dir = session_root.join("sessions").join(&sid);

    // Collect args to forward: everything after the subcommand verb.
    // spawn_daemon() injects '<subcommand> --daemon-child --session-id <ID>' itself.
    // We find the subcommand by position (not substring) to handle global flags
    // that may appear before the subcommand (e.g. `csa --format json review ...`).
    let all_args: Vec<String> = std::env::args().collect();
    let run_pos = all_args.iter().position(|a| a == subcommand).unwrap_or(1);
    let forwarded_args: Vec<String> = all_args
        .iter()
        .skip(run_pos + 1)
        .filter(|a| *a != "--daemon") // daemon is now default; strip no-op flag
        .cloned()
        .collect();

    let csa_binary = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("csa"));
    let mut daemon_env = std::collections::HashMap::new();
    daemon_env.insert("CSA_DAEMON_SESSION_ID".to_string(), sid.clone());
    daemon_env.insert(
        "CSA_DAEMON_SESSION_DIR".to_string(),
        session_dir.display().to_string(),
    );
    daemon_env.insert(
        "CSA_DAEMON_PROJECT_ROOT".to_string(),
        project_root.display().to_string(),
    );

    let config = csa_process::daemon::DaemonSpawnConfig {
        session_id: sid.clone(),
        session_dir: session_dir.clone(),
        csa_binary,
        subcommand: subcommand.to_string(),
        args: forwarded_args,
        env: daemon_env,
    };

    let result = csa_process::daemon::spawn_daemon(config)?;
    // stdout: machine-readable session ID (for script capture).
    println!("{}", result.session_id);
    // stderr: structured RPJ directive for orchestrators.
    // Include --cd <project_root> so cross-project callers can find the session.
    let cd_hint = format!(" --cd '{}'", project_root.display());
    eprintln!(
        "<!-- CSA:SESSION_STARTED id={id} pid={pid} dir=\"{dir}\" \
         wait_cmd=\"csa session wait --session {id}{cd}\" \
         attach_cmd=\"csa session attach --session {id}{cd}\" -->",
        id = result.session_id,
        pid = result.pid,
        dir = result.session_dir.display(),
        cd = cd_hint,
    );
    eprintln!(
        "<!-- CSA:CALLER_HINT action=\"wait\" \
         rule=\"Call 'csa session wait --session {id}{cd}' in a SEPARATE Bash call. \
         NEVER batch multiple waits in a for/while loop. \
         Each wait returns periodically so you can generate tokens and keep your KV cache warm.\" -->",
        id = result.session_id,
        cd = cd_hint,
    );
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    std::process::exit(0);
}
