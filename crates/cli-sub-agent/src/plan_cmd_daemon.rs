//! Daemon spawn + child execution for `csa plan run` (default mode).
//!
//! Mirrors `run_cmd_daemon.rs` but for the `plan run` nested subcommand path.
//! Default behavior is to fork a detached child that runs the workflow in the
//! background; the parent prints the session ULID and exits 0. The caller
//! recovers progress via `csa session wait`/`session result`.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use anyhow::Result;
use chrono::Utc;
use csa_session::{
    MetaSessionState, PhaseEvent, SessionArtifact, SessionResult, save_result, save_session,
};
use tracing::warn;

use crate::cli::PlanCommands;
use crate::pipeline::determine_project_root;
use crate::plan_cmd::{self, PlanRunArgs, handle_plan_run};
use crate::{error_hints, error_report, exit_current_process};

const PLAN_TASK_TYPE: &str = "plan";

/// Dispatch entry point for the `csa plan` subcommand group.
///
/// Routes between three control flows: daemon-child execution (re-exec
/// invariant), foreground inline run (forced for `--dry-run`/`--chunked`/
/// `--resume` or explicit `--foreground`), and the default daemon spawn.
pub(crate) async fn dispatch(
    cmd: PlanCommands,
    current_depth: u32,
    sa_mode_active: bool,
    text_output: bool,
) -> Result<()> {
    let PlanCommands::Run {
        file,
        pattern,
        sa_mode: _,
        vars,
        tool,
        dry_run,
        chunked,
        resume,
        cd,
        foreground,
        daemon_child,
        session_id,
    } = cmd;
    let plan_args = PlanRunArgs {
        file,
        pattern,
        vars,
        tool_override: tool,
        dry_run,
        chunked,
        resume,
        cd,
        current_depth,
    };
    if daemon_child {
        let sid = session_id.ok_or_else(|| {
            anyhow::anyhow!("--daemon-child requires --session-id (set by daemon parent)")
        })?;
        let exit_code = match handle_plan_run_daemon_child(plan_args, &sid).await {
            Ok(code) => code,
            Err(err) => {
                eprintln!("{}", error_report::render_user_facing_error(&err));
                if let Some(hint) = error_hints::suggest_fix(&err) {
                    eprintln!();
                    eprintln!("{hint}");
                }
                1
            }
        };
        crate::pipeline::prompt_guard::emit_sa_mode_caller_guard(
            sa_mode_active,
            current_depth,
            text_output,
        );
        exit_current_process(exit_code);
    }

    if session_id.is_some() {
        anyhow::bail!("--session-id is an internal flag and must not be used directly");
    }

    // --dry-run/--chunked/--resume need synchronous stdout (printed plan,
    // JSON status, awaiting-user prompts), so they force foreground.
    let needs_foreground =
        foreground || plan_args.dry_run || plan_args.chunked || plan_args.resume.is_some();

    if !needs_foreground {
        spawn_and_exit(&plan_args)?;
        unreachable!("plan daemon spawn returned without exiting");
    }

    plan_cmd::handle_plan_run(plan_args).await?;
    crate::pipeline::prompt_guard::emit_sa_mode_caller_guard(
        sa_mode_active,
        current_depth,
        text_output,
    );
    Ok(())
}

/// Spawn a daemon child for `csa plan run` and **never return on success**.
///
/// Writes a placeholder session record (task_type="plan") with the daemon-
/// preassigned session ID, forks via `csa_process::daemon::spawn_daemon`,
/// prints the ULID to stdout and an RPJ directive to stderr, then exits 0.
///
/// On the daemon-child path, [`handle_plan_run_daemon_child`] takes over.
pub(crate) fn spawn_and_exit(args: &PlanRunArgs) -> Result<()> {
    let session_id = csa_session::new_session_id();
    let project_root = determine_project_root(args.cd.as_deref())?;
    let session_root = csa_session::get_session_root(&project_root)?;
    let session_dir = session_root.join("sessions").join(&session_id);

    let description = describe_plan_run(args);
    persist_placeholder_plan_session(&project_root, &session_dir, &session_id, &description)?;

    let forwarded_args = build_forwarded_plan_args(&std::env::args().collect::<Vec<_>>());

    let csa_binary = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("csa"));
    let mut daemon_env = HashMap::new();
    daemon_env.insert("CSA_DAEMON_SESSION_ID".to_string(), session_id.clone());
    daemon_env.insert(
        "CSA_DAEMON_SESSION_DIR".to_string(),
        session_dir.display().to_string(),
    );
    daemon_env.insert(
        "CSA_DAEMON_PROJECT_ROOT".to_string(),
        project_root.display().to_string(),
    );

    let config = csa_process::daemon::DaemonSpawnConfig {
        session_id: session_id.clone(),
        session_dir: session_dir.clone(),
        csa_binary,
        // Multi-word subcommand: the daemon spawner injects
        // `--daemon-child --session-id <ID>` after these verbs so the child
        // re-execs as `csa plan run --daemon-child --session-id <ID> <args>`.
        subcommand: "plan run".to_string(),
        args: forwarded_args,
        env: daemon_env,
    };

    let result = csa_process::daemon::spawn_daemon(config)?;
    println!("{}", result.session_id);
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

/// Daemon-child path: ensure pre-assigned session env is wired so nested
/// `csa run` / `csa review` / `csa debate` invocations attribute their
/// genealogy parent to the plan session, run the workflow inline, then
/// persist `result.toml` and retire the session.
pub(crate) async fn handle_plan_run_daemon_child(
    args: PlanRunArgs,
    session_id: &str,
) -> Result<i32> {
    // SAFETY: the daemon child sets process-scoped env BEFORE async worker
    // tasks rely on it (mirrors run_cmd_daemon flow).
    unsafe { std::env::set_var("CSA_DAEMON_SESSION_ID", session_id) };
    crate::session_cmds_daemon::seed_daemon_session_env(session_id, args.cd.as_deref());
    // Genealogy: nested csa run/review/debate inside this plan session must
    // attribute their parent to the plan session ULID.
    // SAFETY: see comment above; only mutated before tokio worker tasks.
    unsafe { std::env::set_var("CSA_SESSION_ID", session_id) };

    let project_root = determine_project_root(args.cd.as_deref())?;
    let started_at = Utc::now();
    let workflow_label = describe_plan_run(&args);

    // Promote task_type on the placeholder session created by the parent.
    if let Err(err) = mark_session_as_plan(&project_root, session_id, &workflow_label) {
        warn!(
            session_id = %session_id,
            error = %err,
            "Failed to promote placeholder plan session task_type",
        );
    }

    let result = handle_plan_run(args).await;
    let completed_at = Utc::now();
    let exit_code = if result.is_ok() { 0 } else { 1 };
    let status = SessionResult::status_from_exit_code(exit_code);
    let summary = match &result {
        Ok(()) => format!("plan complete: {workflow_label}"),
        Err(err) => {
            let mut text = format!("plan failed: {workflow_label}: {err}");
            text.truncate(
                text.char_indices()
                    .nth(200)
                    .map(|(i, _)| i)
                    .unwrap_or(text.len()),
            );
            text
        }
    };

    let session_result = SessionResult {
        status,
        exit_code,
        summary,
        tool: PLAN_TASK_TYPE.to_string(),
        started_at,
        completed_at,
        events_count: 0,
        artifacts: vec![SessionArtifact::new(workflow_label.clone())],
        peak_memory_mb: None,
        manager_fields: Default::default(),
    };
    if let Err(save_err) = save_result(&project_root, session_id, &session_result) {
        warn!(
            session_id = %session_id,
            error = %save_err,
            "Failed to write plan session result.toml",
        );
    }

    if let Err(err) = retire_plan_session(&project_root, session_id) {
        warn!(
            session_id = %session_id,
            error = %err,
            "Failed to retire plan session phase",
        );
    }

    match result {
        Ok(()) => Ok(0),
        Err(err) => Err(err),
    }
}

fn describe_plan_run(args: &PlanRunArgs) -> String {
    if let Some(name) = &args.pattern {
        format!("plan: {name}")
    } else if let Some(file) = &args.file {
        format!("plan: {file}")
    } else if let Some(resume) = &args.resume {
        format!("plan: --resume {resume}")
    } else {
        "plan: (unknown workflow)".to_string()
    }
}

fn persist_placeholder_plan_session(
    project_root: &Path,
    session_dir: &Path,
    session_id: &str,
    description: &str,
) -> Result<()> {
    let mut state = csa_session::create_session_with_daemon_env(
        project_root,
        Some(description),
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
    state.task_context.task_type = Some(PLAN_TASK_TYPE.to_string());
    if let Err(err) = save_session(&state) {
        warn!(
            session_id = %session_id,
            error = %err,
            "Failed to persist task_type=plan on placeholder session",
        );
    }
    Ok(())
}

fn mark_session_as_plan(project_root: &Path, session_id: &str, description: &str) -> Result<()> {
    let mut session = csa_session::load_session(project_root, session_id)?;
    let mut changed = false;
    if session.task_context.task_type.as_deref() != Some(PLAN_TASK_TYPE) {
        session.task_context.task_type = Some(PLAN_TASK_TYPE.to_string());
        changed = true;
    }
    if session
        .description
        .as_deref()
        .map(str::is_empty)
        .unwrap_or(true)
    {
        session.description = Some(description.to_string());
        changed = true;
    }
    if changed {
        save_session(&session)?;
    }
    Ok(())
}

fn retire_plan_session(project_root: &Path, session_id: &str) -> Result<()> {
    let mut session: MetaSessionState = csa_session::load_session(project_root, session_id)?;
    session.last_accessed = Utc::now();
    if session.phase != csa_session::SessionPhase::Retired
        && session.apply_phase_event(PhaseEvent::Retired).is_err()
    {
        // From Available the transition is also valid; log and continue if unexpected.
        warn!(
            session_id = %session_id,
            current_phase = ?session.phase,
            "Could not transition plan session to Retired",
        );
    }
    save_session(&session)?;
    Ok(())
}

/// Build daemon-child args from the parent's argv.
///
/// `argv` looks like `["csa", ...global, "plan", "run", ...rest]`. We strip
/// everything up through `plan run`, drop `--foreground` (the child is the
/// actual worker, not a separate one), and forward the remainder. The daemon
/// spawner re-injects `--daemon-child --session-id <ID>` between `run` and
/// the rest.
fn build_forwarded_plan_args(all_args: &[String]) -> Vec<String> {
    let plan_pos = all_args.iter().position(|a| a == "plan");
    let Some(plan_pos) = plan_pos else {
        return Vec::new();
    };
    // Skip `plan` and the immediately-following `run` verb.
    let after_plan = plan_pos + 1;
    let after_run = all_args
        .iter()
        .enumerate()
        .skip(after_plan)
        .find(|(_, a)| *a == "run")
        .map(|(idx, _)| idx + 1)
        .unwrap_or(after_plan);

    all_args
        .iter()
        .skip(after_run)
        .filter(|a| *a != "--foreground")
        .cloned()
        .collect()
}

#[cfg(test)]
#[path = "plan_cmd_daemon_tests.rs"]
mod tests;
