//! Daemon spawn logic for execution commands (daemon mode is the default).
//! Shared by `csa run`, `csa review`, and `csa debate`.

use std::io::Write;

use anyhow::Result;

/// Check daemon flags and either spawn+exit or propagate session ID.
///
/// Returns `Ok(())` when the caller should proceed with the child path.
/// **Never returns** when daemon spawn succeeds (calls `process::exit(0)`).
pub(crate) fn check_daemon_flags(
    subcommand: &str,
    no_daemon: bool,
    daemon_child: bool,
    session_id: &Option<String>,
    cd: Option<&str>,
) -> Result<()> {
    if !no_daemon && !daemon_child {
        if session_id.is_some() {
            anyhow::bail!("--session-id is an internal flag and must not be used directly");
        }
        spawn_and_exit(subcommand, cd)?;
    }
    if let Some(sid) = session_id {
        // SAFETY: Runs in the daemon child before tokio spawns worker threads.
        unsafe { std::env::set_var("CSA_DAEMON_SESSION_ID", sid) };
        crate::session_cmds_daemon::seed_daemon_session_env(sid, cd);
    }
    Ok(())
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
