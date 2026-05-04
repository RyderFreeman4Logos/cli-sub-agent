//! Daemon spawn logic for execution commands (daemon mode is the default).
//! Shared by `csa run`, `csa review`, and `csa debate`.

use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const STDIN_PROMPT_MAX_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DaemonSpawnOptions {
    run_stdin_prompt: RunStdinPrompt,
    no_fs_sandbox: bool,
    extra_writable: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum RunStdinPrompt {
    #[default]
    None,
    Omitted,
    PositionalSentinel,
    PromptFileSentinel,
}

impl DaemonSpawnOptions {
    pub(crate) fn for_run(
        skill: Option<&str>,
        prompt: Option<&str>,
        prompt_flag: Option<&str>,
        prompt_file: Option<&Path>,
        no_fs_sandbox: bool,
        extra_writable: &[PathBuf],
    ) -> Self {
        let run_stdin_prompt =
            if prompt_file.is_some_and(crate::run_helpers::is_prompt_file_stdin_sentinel) {
                RunStdinPrompt::PromptFileSentinel
            } else if prompt_file.is_some() || prompt_flag.is_some() {
                RunStdinPrompt::None
            } else if prompt == Some("-") {
                RunStdinPrompt::PositionalSentinel
            } else if prompt.is_none() && skill.is_none() {
                RunStdinPrompt::Omitted
            } else {
                RunStdinPrompt::None
            };

        Self {
            run_stdin_prompt,
            no_fs_sandbox,
            extra_writable: extra_writable.to_vec(),
        }
    }

    pub(crate) fn for_prompt_file(prompt_file: Option<&Path>) -> Self {
        let run_stdin_prompt =
            if prompt_file.is_some_and(crate::run_helpers::is_prompt_file_stdin_sentinel) {
                RunStdinPrompt::PromptFileSentinel
            } else {
                RunStdinPrompt::None
            };

        Self {
            run_stdin_prompt,
            ..Default::default()
        }
    }
}

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
    spawn_options: DaemonSpawnOptions,
) -> Result<DaemonChildGuard> {
    if !no_daemon && !daemon_child {
        if session_id.is_some() {
            anyhow::bail!("--session-id is an internal flag and must not be used directly");
        }
        spawn_and_exit(subcommand, cd, spawn_options)?;
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
    let (max_bytes, keep_rotated, drain_timeout) = resolve_stderr_spool_config(&project_root);

    match csa_process::daemon_stderr::install_stderr_rotation(
        &stderr_path,
        max_bytes,
        keep_rotated,
        drain_timeout,
    ) {
        Ok(guard) => Some(guard),
        Err(e) => {
            // Best effort: log to the original stderr.log before fd replacement.
            eprintln!("[csa] failed to install stderr rotation: {e}");
            None
        }
    }
}

fn resolve_stderr_spool_config(project_root: &std::path::Path) -> (u64, bool, std::time::Duration) {
    let project_cfg = csa_config::ProjectConfig::load(project_root).ok().flatten();

    if let Some(cfg) = project_cfg {
        let max_mb = cfg.session.resolved_stderr_spool_max_mb();
        let max_bytes = u64::from(max_mb).saturating_mul(1024 * 1024);
        let keep = cfg.session.resolved_spool_keep_rotated();
        let drain_timeout = cfg.session.resolved_stderr_drain_timeout();
        return (max_bytes, keep, drain_timeout);
    }

    (
        csa_process::daemon_stderr::DEFAULT_STDERR_SPOOL_MAX_BYTES,
        csa_process::DEFAULT_SPOOL_KEEP_ROTATED,
        std::time::Duration::from_secs(csa_process::daemon_stderr::DEFAULT_DRAIN_TIMEOUT_SECS),
    )
}

/// Fork a daemon child and exit the parent process.
///
/// The daemon child will re-exec with `--daemon-child --session-id <ID>`
/// and the same flags the parent received. stdout.log / stderr.log are
/// captured in the session directory.
///
/// This function **never returns on success** — it calls `process::exit(0)`.
pub(crate) fn spawn_and_exit(
    subcommand: &str,
    cd: Option<&str>,
    spawn_options: DaemonSpawnOptions,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd)?;
    if subcommand == "run" {
        validate_run_daemon_writable_sources(&project_root, &spawn_options)?;
    }
    let sid = csa_session::new_session_id();
    let session_root = csa_session::get_session_root(&project_root)?;
    let session_dir = session_root.join("sessions").join(&sid);
    let stdin_prompt = read_stdin_prompt_if_needed(&spawn_options)?;
    persist_daemon_placeholder_session(&project_root, &session_dir, &sid, subcommand)?;
    let stdin_prompt_file = write_stdin_prompt_if_needed(&session_dir, stdin_prompt)?;

    // Collect args to forward: everything after the subcommand verb.
    // spawn_daemon() injects '<subcommand> --daemon-child --session-id <ID>' itself.
    // We find the subcommand by position (not substring) to handle global flags
    // that may appear before the subcommand (e.g. `csa --format json review ...`).
    let all_args: Vec<String> = std::env::args().collect();
    let forwarded_args = build_forwarded_args(
        &all_args,
        subcommand,
        &spawn_options,
        stdin_prompt_file.as_deref(),
    );

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
         rule=\"Call 'csa session wait --session {id}{cd}' with run_in_background: true. \
         The task-notification IS your wake signal — do NOT stack ScheduleWakeup, /loop, or sleep loops on top. \
         NEVER batch multiple waits in a for/while loop; use one backgrounded Bash tool call per session.\" -->",
        id = result.session_id,
        cd = cd_hint,
    );
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    std::process::exit(0);
}

fn validate_run_daemon_writable_sources(
    project_root: &Path,
    spawn_options: &DaemonSpawnOptions,
) -> Result<()> {
    let config = csa_config::ProjectConfig::load(project_root)?;
    crate::pipeline_sandbox::validate_run_extra_writable_sources_exist(
        config.as_ref(),
        project_root,
        spawn_options.no_fs_sandbox,
        &spawn_options.extra_writable,
    )
    .map_err(anyhow::Error::msg)
}

fn persist_daemon_placeholder_session(
    project_root: &Path,
    session_dir: &Path,
    session_id: &str,
    subcommand: &str,
) -> Result<()> {
    let state = csa_session::create_session_with_daemon_env(
        project_root,
        Some(&format!("initializing daemon {subcommand}")),
        None,
        None,
        Some(session_id),
        Some(session_dir),
        Some(project_root),
    )?;
    anyhow::ensure!(
        state.meta_session_id == session_id,
        "daemon placeholder session id mismatch: requested {session_id}, persisted {}",
        state.meta_session_id
    );
    Ok(())
}

fn read_stdin_prompt_if_needed(spawn_options: &DaemonSpawnOptions) -> Result<Option<String>> {
    if spawn_options.run_stdin_prompt == RunStdinPrompt::None {
        return Ok(None);
    }

    let mut stdin = std::io::stdin();
    if stdin.is_terminal() {
        anyhow::bail!(
            "No prompt provided and stdin is a terminal.\n\n\
             Usage:\n  \
             csa run --sa-mode <true|false> --tool <tool> \"your prompt here\"\n  \
             echo \"prompt\" | csa run --sa-mode <true|false> --tool <tool>"
        );
    }

    let prompt = read_bounded_stdin_prompt(&mut stdin, STDIN_PROMPT_MAX_BYTES)
        .context("failed to read daemon run prompt from stdin")?;
    if prompt.trim().is_empty() {
        anyhow::bail!("Empty prompt from stdin. Provide a non-empty prompt.");
    }
    Ok(Some(prompt))
}

fn read_bounded_stdin_prompt(reader: impl Read, max_bytes: u64) -> Result<String> {
    let mut prompt = String::new();
    let mut limited_reader = reader.take(max_bytes.saturating_add(1));
    limited_reader.read_to_string(&mut prompt)?;
    if prompt.len() as u64 > max_bytes {
        anyhow::bail!(
            "Prompt from stdin exceeds the {} byte daemon limit. Use --prompt-file for larger input.",
            max_bytes
        );
    }
    Ok(prompt)
}

fn write_stdin_prompt_if_needed(
    session_dir: &Path,
    prompt: Option<String>,
) -> Result<Option<PathBuf>> {
    let Some(prompt) = prompt else {
        return Ok(None);
    };
    let input_dir = session_dir.join("input");
    std::fs::create_dir_all(&input_dir).with_context(|| {
        format!(
            "failed to create daemon prompt input dir {}",
            input_dir.display()
        )
    })?;
    let prompt_path = input_dir.join("stdin-prompt.txt");
    std::fs::write(&prompt_path, prompt).with_context(|| {
        format!(
            "failed to write daemon stdin prompt file {}",
            prompt_path.display()
        )
    })?;
    Ok(Some(prompt_path))
}

fn build_forwarded_args(
    all_args: &[String],
    subcommand: &str,
    spawn_options: &DaemonSpawnOptions,
    stdin_prompt_file: Option<&Path>,
) -> Vec<String> {
    let run_pos = all_args.iter().position(|a| a == subcommand).unwrap_or(1);
    let mut forwarded_args: Vec<String> = all_args
        .iter()
        .skip(run_pos + 1)
        .filter(|a| *a != "--daemon")
        .cloned()
        .collect();

    if spawn_options.run_stdin_prompt == RunStdinPrompt::PositionalSentinel
        && let Some(pos) = forwarded_args.iter().rposition(|arg| arg == "-")
    {
        forwarded_args.remove(pos);
        if forwarded_args.last().is_some_and(|arg| arg == "--") {
            forwarded_args.pop();
        }
    }

    if spawn_options.run_stdin_prompt == RunStdinPrompt::PromptFileSentinel {
        remove_prompt_file_sentinel_arg(&mut forwarded_args);
    }

    if let Some(prompt_file) = stdin_prompt_file {
        forwarded_args.push("--prompt-file".to_string());
        forwarded_args.push(prompt_file.display().to_string());
    }

    forwarded_args
}

fn remove_prompt_file_sentinel_arg(args: &mut Vec<String>) {
    let Some(pos) = args.iter().position(|arg| {
        arg.strip_prefix("--prompt-file=").is_some_and(|value| {
            crate::run_helpers::is_prompt_file_stdin_sentinel(Path::new(value))
        }) || arg == "--prompt-file"
    }) else {
        return;
    };

    if args[pos] == "--prompt-file" {
        if args.get(pos + 1).is_some_and(|value| {
            crate::run_helpers::is_prompt_file_stdin_sentinel(Path::new(value))
        }) {
            args.drain(pos..=pos + 1);
        }
    } else {
        args.remove(pos);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_daemon_options_detect_omitted_stdin_prompt_without_skill() {
        let options = DaemonSpawnOptions::for_run(None, None, None, None, false, &[]);
        assert_eq!(options.run_stdin_prompt, RunStdinPrompt::Omitted);
    }

    #[test]
    fn run_daemon_options_do_not_capture_stdin_for_skill_only_run() {
        let options = DaemonSpawnOptions::for_run(Some("demo"), None, None, None, false, &[]);
        assert_eq!(options.run_stdin_prompt, RunStdinPrompt::None);
    }

    #[test]
    fn run_daemon_options_detect_positional_stdin_sentinel() {
        let options = DaemonSpawnOptions::for_run(None, Some("-"), None, None, false, &[]);
        assert_eq!(options.run_stdin_prompt, RunStdinPrompt::PositionalSentinel);
    }

    #[test]
    fn run_daemon_options_detect_prompt_file_stdin_sentinel() {
        let options =
            DaemonSpawnOptions::for_run(None, None, None, Some(Path::new("-")), false, &[]);
        assert_eq!(options.run_stdin_prompt, RunStdinPrompt::PromptFileSentinel);
    }

    #[test]
    fn prompt_file_daemon_options_detect_dev_stdin() {
        let options = DaemonSpawnOptions::for_prompt_file(Some(Path::new("/dev/stdin")));
        assert_eq!(options.run_stdin_prompt, RunStdinPrompt::PromptFileSentinel);
    }

    #[test]
    fn forwarded_args_append_prompt_file_for_omitted_stdin_prompt() {
        let all_args = vec![
            "csa".to_string(),
            "run".to_string(),
            "--sa-mode".to_string(),
            "true".to_string(),
        ];
        let prompt_file = Path::new("/state/session/input/stdin-prompt.txt");

        let forwarded = build_forwarded_args(
            &all_args,
            "run",
            &DaemonSpawnOptions {
                run_stdin_prompt: RunStdinPrompt::Omitted,
                ..Default::default()
            },
            Some(prompt_file),
        );

        assert_eq!(
            forwarded,
            vec![
                "--sa-mode",
                "true",
                "--prompt-file",
                "/state/session/input/stdin-prompt.txt"
            ]
        );
    }

    #[test]
    fn forwarded_args_replace_positional_stdin_sentinel_with_prompt_file() {
        let all_args = vec![
            "csa".to_string(),
            "run".to_string(),
            "--sa-mode".to_string(),
            "true".to_string(),
            "-".to_string(),
        ];
        let prompt_file = Path::new("/state/session/input/stdin-prompt.txt");

        let forwarded = build_forwarded_args(
            &all_args,
            "run",
            &DaemonSpawnOptions {
                run_stdin_prompt: RunStdinPrompt::PositionalSentinel,
                ..Default::default()
            },
            Some(prompt_file),
        );

        assert_eq!(
            forwarded,
            vec![
                "--sa-mode",
                "true",
                "--prompt-file",
                "/state/session/input/stdin-prompt.txt"
            ]
        );
    }

    #[test]
    fn forwarded_args_replace_prompt_file_stdin_sentinel_with_prompt_file() {
        let all_args = vec![
            "csa".to_string(),
            "debate".to_string(),
            "--sa-mode".to_string(),
            "true".to_string(),
            "--prompt-file".to_string(),
            "/dev/stdin".to_string(),
        ];
        let prompt_file = Path::new("/state/session/input/stdin-prompt.txt");

        let forwarded = build_forwarded_args(
            &all_args,
            "debate",
            &DaemonSpawnOptions {
                run_stdin_prompt: RunStdinPrompt::PromptFileSentinel,
                ..Default::default()
            },
            Some(prompt_file),
        );

        assert_eq!(
            forwarded,
            vec![
                "--sa-mode",
                "true",
                "--prompt-file",
                "/state/session/input/stdin-prompt.txt"
            ]
        );
    }

    #[test]
    fn forwarded_args_replace_prompt_file_equals_stdin_sentinel_with_prompt_file() {
        let all_args = vec![
            "csa".to_string(),
            "run".to_string(),
            "--sa-mode".to_string(),
            "true".to_string(),
            "--prompt-file=-".to_string(),
        ];
        let prompt_file = Path::new("/state/session/input/stdin-prompt.txt");

        let forwarded = build_forwarded_args(
            &all_args,
            "run",
            &DaemonSpawnOptions {
                run_stdin_prompt: RunStdinPrompt::PromptFileSentinel,
                ..Default::default()
            },
            Some(prompt_file),
        );

        assert_eq!(
            forwarded,
            vec![
                "--sa-mode",
                "true",
                "--prompt-file",
                "/state/session/input/stdin-prompt.txt"
            ]
        );
    }

    #[test]
    fn forwarded_args_remove_trailing_double_dash_with_stdin_sentinel() {
        let all_args = vec![
            "csa".to_string(),
            "run".to_string(),
            "--sa-mode".to_string(),
            "true".to_string(),
            "--".to_string(),
            "-".to_string(),
        ];
        let prompt_file = Path::new("/state/session/input/stdin-prompt.txt");

        let forwarded = build_forwarded_args(
            &all_args,
            "run",
            &DaemonSpawnOptions {
                run_stdin_prompt: RunStdinPrompt::PositionalSentinel,
                ..Default::default()
            },
            Some(prompt_file),
        );

        assert_eq!(
            forwarded,
            vec![
                "--sa-mode",
                "true",
                "--prompt-file",
                "/state/session/input/stdin-prompt.txt"
            ]
        );
    }

    #[test]
    fn bounded_stdin_prompt_accepts_prompt_at_limit() {
        let prompt = "x".repeat(16);
        let read = read_bounded_stdin_prompt(std::io::Cursor::new(prompt.as_bytes()), 16)
            .expect("prompt at limit should be accepted");

        assert_eq!(read, prompt);
    }

    #[test]
    fn bounded_stdin_prompt_rejects_prompt_over_limit() {
        let prompt = "x".repeat(17);
        let err = read_bounded_stdin_prompt(std::io::Cursor::new(prompt.as_bytes()), 16)
            .expect_err("prompt over limit should fail");

        assert!(
            err.to_string().contains("exceeds the 16 byte daemon limit"),
            "unexpected error: {err}"
        );
    }
}
