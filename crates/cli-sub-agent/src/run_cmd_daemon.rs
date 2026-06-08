//! Daemon spawn logic for execution commands (daemon mode is the default).
//! Shared by `csa run`, `csa review`, and `csa debate`.

use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::debate_errors::EMPTY_DEBATE_QUESTION_ERROR;
use crate::startup_env::StartupSubtreeEnv;

const STDIN_PROMPT_MAX_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DaemonSpawnOptions {
    run_stdin_prompt: RunStdinPrompt,
    prompt_file_to_capture: Option<PathBuf>,
    remove_prompt_file_arg: bool,
    prompt_file_forward_arg: PromptFileForwardArg,
    no_fs_sandbox: bool,
    extra_writable: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum RunStdinPrompt {
    #[default]
    None,
    Omitted,
    DebateOmitted,
    PositionalSentinel,
    PromptFileSentinel,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum PromptFileForwardArg {
    #[default]
    PromptFile,
    QuestionFile,
}

impl PromptFileForwardArg {
    fn flag(self) -> &'static str {
        match self {
            Self::PromptFile => "--prompt-file",
            Self::QuestionFile => "--question-file",
        }
    }

    fn accepted_flags(self) -> &'static [&'static str] {
        match self {
            Self::PromptFile => &["--prompt-file"],
            Self::QuestionFile => &["--question-file", "--prompt-file"],
        }
    }
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
            prompt_file_forward_arg: PromptFileForwardArg::PromptFile,
            no_fs_sandbox,
            extra_writable: extra_writable.to_vec(),
            ..Default::default()
        }
    }

    pub(crate) fn for_debate(
        question: Option<&str>,
        topic: Option<&str>,
        question_file: Option<&Path>,
    ) -> Self {
        let question_from_stdin = question == Some("-");
        let inline_question_available = question.is_some() || topic.is_some();
        let question_file_is_stdin =
            question_file.is_some_and(crate::run_helpers::is_prompt_file_stdin_sentinel);
        let run_stdin_prompt = if question_from_stdin {
            RunStdinPrompt::PositionalSentinel
        } else if inline_question_available {
            RunStdinPrompt::None
        } else if question_file_is_stdin {
            RunStdinPrompt::PromptFileSentinel
        } else if question_file.is_none() {
            RunStdinPrompt::DebateOmitted
        } else {
            RunStdinPrompt::None
        };
        let prompt_file_to_capture = if !inline_question_available && !question_file_is_stdin {
            question_file.map(Path::to_path_buf)
        } else {
            None
        };
        let remove_prompt_file_arg = question_from_stdin && question_file.is_some();

        Self {
            run_stdin_prompt,
            prompt_file_to_capture,
            remove_prompt_file_arg,
            prompt_file_forward_arg: PromptFileForwardArg::QuestionFile,
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
    startup_env: &mut StartupSubtreeEnv,
    spawn_options: DaemonSpawnOptions,
) -> Result<DaemonChildGuard> {
    if !no_daemon && !daemon_child {
        if session_id.is_some() {
            anyhow::bail!("--session-id is an internal flag and must not be used directly");
        }
        spawn_and_exit(subcommand, cd, startup_env, spawn_options)?;
    }
    let mut stderr_rotation = None;
    if let Some(sid) = session_id {
        // SAFETY: Runs in the daemon child before tokio spawns worker threads.
        unsafe {
            std::env::set_var("CSA_DAEMON_SESSION_ID", sid);
            std::env::set_var(csa_core::env::CSA_SESSION_ID_ENV_KEY, sid);
        }
        crate::session_cmds_daemon::seed_daemon_session_env(sid, cd);
        // Keep ordinary run/review/debate daemon children in sync with the plan-daemon
        // invariant established by `inject_plan_daemon_session_into_startup_env`: after
        // daemon bootstrap, StartupSubtreeEnv::session_id() names the executing session.
        *startup_env = daemon_child_startup_env(startup_env, sid, cd)?;

        // Install stderr rotation so daemon stderr.log is bounded.
        stderr_rotation = install_daemon_stderr_rotation(sid, cd);

        // Install panic hook so daemon-completion.toml is written even on panic.
        install_daemon_panic_hook();

        // Spawn a background task that writes daemon-completion.toml on SIGTERM/SIGINT.
        // This runs inside the tokio runtime (we are inside `#[tokio::main]`).
        spawn_daemon_signal_handler();
    }
    Ok(DaemonChildGuard {
        _stderr_rotation: stderr_rotation,
    })
}

fn daemon_child_startup_env(
    startup_env: &StartupSubtreeEnv,
    session_id: &str,
    cd: Option<&str>,
) -> Result<StartupSubtreeEnv> {
    let project_root = crate::pipeline::determine_project_root(cd)?;
    let session_dir = csa_session::get_session_dir(&project_root, session_id)?;
    Ok(startup_env
        .clone()
        .with_current_session(session_id, session_dir.display().to_string()))
}

/// Install a custom panic hook that writes `daemon-completion.toml` before
/// chaining to the default hook (preserving backtrace output).
///
/// Exit code 101 is Rust's conventional panic exit code.
fn install_daemon_panic_hook() {
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        crate::session_cmds_daemon::persist_daemon_completion_from_env(101);
        prev_hook(info);
    }));
}

/// Spawn a tokio task that listens for SIGTERM and SIGINT, writes
/// `daemon-completion.toml` with the conventional signal exit code
/// (128 + signal number), then exits.
fn spawn_daemon_signal_handler() {
    tokio::spawn(async {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let mut sigterm = match signal(SignalKind::terminate()) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("failed to register daemon SIGTERM handler: {e}");
                    return;
                }
            };
            let mut sigint = match signal(SignalKind::interrupt()) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("failed to register daemon SIGINT handler: {e}");
                    return;
                }
            };
            tokio::select! {
                _ = sigterm.recv() => {
                    // 128 + 15 (SIGTERM) = 143
                    crate::session_cmds_daemon::persist_daemon_completion_from_env(143);
                    std::process::exit(143);
                }
                _ = sigint.recv() => {
                    // 128 + 2 (SIGINT) = 130
                    crate::session_cmds_daemon::persist_daemon_completion_from_env(130);
                    std::process::exit(130);
                }
            }
        }
    });
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
    startup_env: &StartupSubtreeEnv,
    spawn_options: DaemonSpawnOptions,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd)?;
    if subcommand == "run" {
        validate_run_daemon_writable_sources(&project_root, &spawn_options)?;
    }
    let sid = csa_session::new_session_id();
    let session_root = csa_session::get_session_root(&project_root)?;
    let session_dir = session_root.join("sessions").join(&sid);
    let prompt_input = read_daemon_prompt_input_if_needed(&spawn_options)?;
    persist_daemon_placeholder_session(&project_root, &session_dir, &sid, subcommand)?;
    let prompt_input_file = write_daemon_prompt_input_if_needed(&session_dir, prompt_input)?;

    // Collect args to forward: everything after the subcommand verb.
    // spawn_daemon() injects '<subcommand> --daemon-child --session-id <ID>' itself.
    // We find the subcommand by position (not substring) to handle global flags
    // that may appear before the subcommand (e.g. `csa --format json review ...`).
    let all_args: Vec<String> = std::env::args().collect();
    let forwarded_args = build_forwarded_args(
        &all_args,
        subcommand,
        &spawn_options,
        prompt_input_file.as_deref(),
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
    startup_env.apply_to_child_env(&mut daemon_env);

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
    let wait_cmd =
        crate::daemon_caller_hints::format_session_wait_command(&result.session_id, &project_root);
    let attach_cmd = crate::daemon_caller_hints::format_session_attach_command(
        &result.session_id,
        &project_root,
    );
    eprintln!(
        "<!-- CSA:SESSION_STARTED id={id} pid={pid} dir=\"{dir}\" \
         wait_cmd=\"{wait_cmd}\" \
         attach_cmd=\"{attach_cmd}\" -->",
        id = result.session_id,
        pid = result.pid,
        dir = result.session_dir.display(),
        wait_cmd = wait_cmd,
        attach_cmd = attach_cmd,
    );
    eprintln!(
        "<!-- CSA:CALLER_HINT action=\"wait\" \
         rule=\"Call {wait_cmd} with run_in_background: true. \
         The task-notification IS your wake signal — do NOT stack ScheduleWakeup, /loop, or sleep loops on top. \
         NEVER batch multiple waits in a for/while loop; use one backgrounded Bash tool call per session. \
         FORBIDDEN after backgrounding: ls/cat/wc/grep on session-dir, state.toml reads, ps checks on daemon PID — \
         any manual polling wastes caller tokens with zero benefit. \
         FORBIDDEN: piping csa commands through 2>/dev/null. CSA errors on stderr are diagnostic — \
         suppressing them hides invalid-argument errors and causes silent retry loops that waste thousands of tokens.\" -->",
        wait_cmd = wait_cmd,
    );
    let codex_hint = crate::process_tree::codex_yield_hint();
    if !codex_hint.is_empty() {
        eprint!("{codex_hint}");
    }
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

fn read_daemon_prompt_input_if_needed(
    spawn_options: &DaemonSpawnOptions,
) -> Result<Option<String>> {
    let mut stdin = std::io::stdin();
    read_daemon_prompt_input_if_needed_from_reader(spawn_options, stdin.is_terminal(), &mut stdin)
}

fn read_daemon_prompt_input_if_needed_from_reader<R: Read>(
    spawn_options: &DaemonSpawnOptions,
    stdin_is_terminal: bool,
    reader: &mut R,
) -> Result<Option<String>> {
    if let Some(path) = &spawn_options.prompt_file_to_capture {
        let prompt = read_daemon_prompt_file(path, spawn_options.prompt_file_forward_arg)?;
        return Ok(Some(prompt));
    }

    if spawn_options.run_stdin_prompt == RunStdinPrompt::None {
        return Ok(None);
    }

    if stdin_is_terminal {
        if spawn_options.run_stdin_prompt == RunStdinPrompt::DebateOmitted {
            anyhow::bail!(EMPTY_DEBATE_QUESTION_ERROR);
        }
        anyhow::bail!(
            "No prompt provided and stdin is a terminal.\n\n\
             Usage:\n  \
             csa run --sa-mode <true|false> --tool <tool> \"your prompt here\"\n  \
             echo \"prompt\" | csa run --sa-mode <true|false> --tool <tool>"
        );
    }

    let read_context = if spawn_options.run_stdin_prompt == RunStdinPrompt::DebateOmitted {
        "failed to read daemon debate question from stdin"
    } else {
        "failed to read daemon run prompt from stdin"
    };
    let prompt = read_bounded_stdin_prompt(reader, STDIN_PROMPT_MAX_BYTES).context(read_context)?;
    if prompt.trim().is_empty() {
        if spawn_options.run_stdin_prompt == RunStdinPrompt::DebateOmitted {
            anyhow::bail!(EMPTY_DEBATE_QUESTION_ERROR);
        }
        anyhow::bail!("Empty prompt from stdin. Provide a non-empty prompt.");
    }
    Ok(Some(prompt))
}

fn read_daemon_prompt_file(path: &Path, forward_arg: PromptFileForwardArg) -> Result<String> {
    let flag = forward_arg.flag();
    let prompt = std::fs::read_to_string(path)
        .with_context(|| format!("{flag}: failed to read '{}'", path.display()))?;
    if prompt.trim().is_empty() {
        anyhow::bail!("{flag} '{}' is empty", path.display());
    }
    Ok(prompt)
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

fn write_daemon_prompt_input_if_needed(
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
        remove_prompt_file_arg(
            &mut forwarded_args,
            spawn_options.prompt_file_forward_arg.accepted_flags(),
            true,
        );
    }

    if spawn_options.prompt_file_to_capture.is_some() {
        remove_prompt_file_arg(
            &mut forwarded_args,
            spawn_options.prompt_file_forward_arg.accepted_flags(),
            false,
        );
    }

    if spawn_options.remove_prompt_file_arg {
        remove_prompt_file_arg(
            &mut forwarded_args,
            spawn_options.prompt_file_forward_arg.accepted_flags(),
            false,
        );
    }

    if let Some(prompt_file) = stdin_prompt_file {
        forwarded_args.push(spawn_options.prompt_file_forward_arg.flag().to_string());
        forwarded_args.push(prompt_file.display().to_string());
    }

    forwarded_args
}

fn remove_prompt_file_arg(args: &mut Vec<String>, flags: &[&str], sentinel_only: bool) {
    let Some((pos, flag, value_in_arg)) = args.iter().enumerate().find_map(|(pos, arg)| {
        flags.iter().find_map(|flag| {
            if arg == flag {
                Some((pos, *flag, None))
            } else {
                arg.strip_prefix(&format!("{flag}="))
                    .map(|value| (pos, *flag, Some(value.to_string())))
            }
        })
    }) else {
        return;
    };

    if let Some(value) = value_in_arg {
        if !sentinel_only || crate::run_helpers::is_prompt_file_stdin_sentinel(Path::new(&value)) {
            args.remove(pos);
        }
        return;
    }

    if args[pos] == flag {
        let Some(value) = args.get(pos + 1) else {
            return;
        };
        if !sentinel_only || crate::run_helpers::is_prompt_file_stdin_sentinel(Path::new(value)) {
            args.drain(pos..=pos + 1);
        }
    } else {
        args.remove(pos);
    }
}

#[cfg(test)]
#[path = "run_cmd_daemon_tests.rs"]
mod tests;
