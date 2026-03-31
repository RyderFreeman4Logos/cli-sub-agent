//! Daemon spawn logic for `csa run --daemon`.
//!
//! Extracted from main.rs to keep the dispatch function under the
//! monolith file limit.

use std::io::Write;

use anyhow::Result;

/// Fork a daemon child and exit the parent process.
///
/// The daemon child will re-exec with `--daemon-child --session-id <ID>`
/// and the same flags the parent received. stdout.log / stderr.log are
/// captured in the session directory.
///
/// This function **never returns on success** — it calls `process::exit(0)`.
pub(crate) fn spawn_and_exit(cd: Option<&str>) -> Result<()> {
    let sid = csa_session::new_session_id();
    let project_root = crate::pipeline::determine_project_root(cd)?;
    let session_root = csa_session::get_session_root(&project_root)?;
    let session_dir = session_root.join("sessions").join(&sid);

    // Collect args to forward: everything after the 'run' subcommand verb.
    // spawn_daemon() injects 'run --daemon-child --session-id <ID>' itself.
    // We find "run" by position (not substring) to handle global flags
    // that may appear before the subcommand (e.g. `csa --format json run ...`).
    let all_args: Vec<String> = std::env::args().collect();
    let run_pos = all_args.iter().position(|a| a == "run").unwrap_or(1);
    let forwarded_args: Vec<String> = all_args.iter().skip(run_pos + 1).cloned().collect();

    let csa_binary = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("csa"));

    let config = csa_process::daemon::DaemonSpawnConfig {
        session_id: sid.clone(),
        session_dir: session_dir.clone(),
        csa_binary,
        args: forwarded_args,
        env: std::collections::HashMap::new(),
    };

    let result = csa_process::daemon::spawn_daemon(config)?;
    // stdout: machine-readable session ID (for script capture).
    println!("{}", result.session_id);
    // stderr: structured RPJ directive for orchestrators.
    eprintln!(
        "<!-- CSA:SESSION_STARTED id={id} pid={pid} dir=\"{dir}\" \
         wait_cmd=\"csa session wait --session {id}\" \
         attach_cmd=\"csa session attach --session {id}\" -->",
        id = result.session_id,
        pid = result.pid,
        dir = result.session_dir.display(),
    );
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    std::process::exit(0);
}
