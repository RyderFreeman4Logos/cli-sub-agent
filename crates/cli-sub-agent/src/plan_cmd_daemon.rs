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
use crate::gh_env::fetch_issue_body;
use crate::pipeline::determine_project_root;
use crate::plan_cmd::{
    self, FEATURE_INPUT_VAR, ISSUE_NUMBER_VAR, PlanRunArgs, PlanRunPipelineSource, handle_plan_run,
};
use crate::startup_env::StartupSubtreeEnv;
use crate::{error_hints, error_report, exit_current_process};

const PLAN_TASK_TYPE: &str = "plan";
const PLAN_PIPELINE_SOURCE_ENV: &str = "CSA_PLAN_PIPELINE_SOURCE";

pub(crate) struct PlanRunDispatchInput {
    pub foreground: bool,
    pub daemon_child: bool,
    pub session_id: Option<String>,
    pub sa_mode_active: bool,
    pub text_output: bool,
    pub forwarded_args: Option<Vec<String>>,
}

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
    startup_env: &StartupSubtreeEnv,
) -> Result<()> {
    let PlanCommands::Run {
        file,
        pattern,
        sa_mode: _,
        vars,
        issue,
        tool,
        model_spec,
        dry_run,
        chunked,
        resume,
        cd,
        no_fs_sandbox,
        foreground,
        daemon_child,
        session_id,
    } = cmd;

    // Resolve `--issue <N>` into workflow variables before the daemon/foreground
    // split. Fetching in the top-level invocation means a bad issue number or
    // auth failure fails fast with a non-zero exit rather than surfacing only
    // via the daemon session result, and the issue is fetched exactly once: the
    // resolved variables are forwarded to the daemon child in place of `--issue`
    // (see `forwarded_args_with_feature_input`). A daemon child never sees
    // `--issue` (the parent strips it), so this branch only runs in the
    // top-level/foreground process.
    let mut vars = vars;
    let mut forwarded_args = None;
    if let Some(issue_number) = issue {
        if vars.iter().any(|entry| {
            entry
                .split_once('=')
                .is_some_and(|(key, _)| key == FEATURE_INPUT_VAR)
        }) {
            anyhow::bail!(
                "--issue {issue_number} conflicts with an explicit --var {FEATURE_INPUT_VAR}=...; \
                 supply only one source for the workflow's {FEATURE_INPUT_VAR} variable"
            );
        }
        let body = fetch_issue_body(issue_number).await?;
        forwarded_args = Some(forwarded_args_with_feature_input(&body, issue_number));
        vars.push(format!("{FEATURE_INPUT_VAR}={body}"));
        vars.push(format!("{ISSUE_NUMBER_VAR}={issue_number}"));
    }

    let pipeline_source = if daemon_child {
        std::env::var(PLAN_PIPELINE_SOURCE_ENV)
            .ok()
            .and_then(|value| PlanRunPipelineSource::from_str(&value))
            .unwrap_or(PlanRunPipelineSource::DirectPlanRun)
    } else {
        PlanRunPipelineSource::DirectPlanRun
    };
    let plan_args = PlanRunArgs {
        file,
        pattern,
        vars,
        tool_override: tool,
        model_spec_override: model_spec,
        dry_run,
        chunked,
        resume,
        cd,
        no_fs_sandbox,
        current_depth,
        pipeline_source,
        startup_env: startup_env.clone(),
    };
    dispatch_plan_run(
        plan_args,
        PlanRunDispatchInput {
            foreground,
            daemon_child,
            session_id,
            sa_mode_active,
            text_output,
            forwarded_args,
        },
    )
    .await
}

pub(crate) async fn dispatch_plan_run(
    plan_args: PlanRunArgs,
    input: PlanRunDispatchInput,
) -> Result<()> {
    let current_depth = plan_args.current_depth;
    if input.daemon_child {
        let sid = input.session_id.ok_or_else(|| {
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
            input.sa_mode_active,
            current_depth,
            input.text_output,
        );
        exit_current_process(exit_code);
    }

    if input.session_id.is_some() {
        anyhow::bail!("--session-id is an internal flag and must not be used directly");
    }

    let needs_foreground = decide_needs_foreground(ForegroundDecisionInput {
        foreground: input.foreground,
        dry_run: plan_args.dry_run,
        chunked: plan_args.chunked,
        has_resume: plan_args.resume.is_some(),
        current_depth,
        nested_env: nested_session_env_present(&plan_args.startup_env),
    });

    if !needs_foreground {
        spawn_and_exit(&plan_args, input.forwarded_args)?;
        unreachable!("plan daemon spawn returned without exiting");
    }

    // Foreground path (nested invocation, `--foreground`, `--resume`, or
    // `--chunked`): unlike the daemon-child path, nothing here has wired a
    // session identity into `startup_env`, so `spawn_bash` would omit
    // CSA_SESSION_DIR / CSA_SESSION_ID from every workflow bash step. A
    // top-level `--foreground` or `--resume` mktd run then dies in its Save
    // step on `${CSA_SESSION_DIR:?...}` (#1851). `--dry-run` only prints the
    // compiled plan and runs no bash steps, so it needs no session scratch dir
    // and must stay side-effect free.
    let mut plan_args = plan_args;
    let foreground_session = if plan_args.dry_run {
        None
    } else {
        let project_root = determine_project_root(plan_args.cd.as_deref())?;
        let established = establish_foreground_plan_session(
            &plan_args.startup_env,
            &project_root,
            &describe_plan_run(&plan_args),
        )?;
        plan_args.startup_env = established.startup_env;
        established
            .minted_session_id
            .map(|session_id| (project_root, session_id))
    };

    let run_result = plan_cmd::handle_plan_run(plan_args).await;

    // Retire only a session this path minted; an inherited (nested) session is
    // owned by the parent and must outlive this call.
    if let Some((project_root, session_id)) = foreground_session
        && let Err(err) = retire_plan_session(&project_root, &session_id)
    {
        warn!(
            session_id = %session_id,
            error = %err,
            "Failed to retire foreground plan session",
        );
    }
    run_result?;

    crate::pipeline::prompt_guard::emit_sa_mode_caller_guard(
        input.sa_mode_active,
        current_depth,
        input.text_output,
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
pub(crate) fn spawn_and_exit(
    args: &PlanRunArgs,
    forwarded_args: Option<Vec<String>>,
) -> Result<()> {
    let session_id = csa_session::new_session_id();
    let project_root = determine_project_root(args.cd.as_deref())?;
    let session_root = csa_session::get_session_root(&project_root)?;
    let session_dir = session_root.join("sessions").join(&session_id);

    let description = describe_plan_run(args);
    persist_placeholder_plan_session(&project_root, &session_dir, &session_id, &description)?;

    let forwarded_args = forwarded_args
        .unwrap_or_else(|| build_forwarded_plan_args(&std::env::args().collect::<Vec<_>>()));

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
    daemon_env.insert(
        PLAN_PIPELINE_SOURCE_ENV.to_string(),
        args.pipeline_source.as_str().to_string(),
    );
    args.startup_env.apply_to_child_env(&mut daemon_env);

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
    let wait_cmd =
        crate::daemon_caller_hints::format_session_wait_command(&result.session_id, &project_root);
    let attach_cmd = crate::daemon_caller_hints::format_session_attach_command(
        &result.session_id,
        &project_root,
    );
    let session_dir_attr = crate::daemon_caller_hints::escape_structured_comment_attr(
        &result.session_dir.display().to_string(),
    );
    let wait_cmd_attr = crate::daemon_caller_hints::escape_structured_comment_attr(&wait_cmd);
    let attach_cmd_attr = crate::daemon_caller_hints::escape_structured_comment_attr(&attach_cmd);
    eprintln!(
        "<!-- CSA:SESSION_STARTED id={id} pid={pid} dir=\"{dir}\" \
         wait_cmd=\"{wait_cmd}\" \
         attach_cmd=\"{attach_cmd}\" -->",
        id = result.session_id,
        pid = result.pid,
        dir = session_dir_attr,
        wait_cmd = wait_cmd_attr,
        attach_cmd = attach_cmd_attr,
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
        wait_cmd = wait_cmd_attr,
    );
    let codex_hint = crate::process_tree::codex_yield_hint();
    if !codex_hint.is_empty() {
        eprint!("{codex_hint}");
    }
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    std::process::exit(0);
}

/// Daemon-child path: ensure pre-assigned session env is wired so nested
/// `csa run` / `csa review` / `csa debate` invocations attribute their
/// genealogy parent to the plan session, run the workflow inline, then
/// persist `result.toml` and retire the session.
pub(crate) async fn handle_plan_run_daemon_child(
    mut args: PlanRunArgs,
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
    inject_plan_daemon_session_into_startup_env(&mut args, session_id, &project_root)?;
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
        post_exec_gate: None,
        status,
        exit_code,
        summary,
        tool: PLAN_TASK_TYPE.to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at,
        completed_at,
        events_count: 0,
        artifacts: vec![SessionArtifact::new(workflow_label.clone())],
        ..Default::default()
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

fn inject_plan_daemon_session_into_startup_env(
    args: &mut PlanRunArgs,
    session_id: &str,
    project_root: &Path,
) -> Result<()> {
    let session_dir = csa_session::get_session_dir(project_root, session_id)?;
    args.startup_env = args
        .startup_env
        .clone()
        .with_current_session(session_id, session_dir.display().to_string());
    Ok(())
}

/// Result of ensuring the foreground `csa plan run` path has a session identity
/// to thread into its workflow bash steps.
///
/// `minted_session_id` is `Some` only when this path created a brand-new
/// placeholder plan session (the top-level `--foreground` / `--resume` case);
/// the caller is then responsible for retiring it once the run completes. When
/// the startup snapshot already carried a session (a nested invocation), the
/// existing identity is reused untouched and `minted_session_id` is `None`.
struct ForegroundPlanSession {
    startup_env: StartupSubtreeEnv,
    minted_session_id: Option<String>,
}

/// Guarantee the foreground plan run carries a session identity (id + dir) in
/// `startup_env` so `spawn_bash` exports CSA_SESSION_ID / CSA_SESSION_DIR to
/// every workflow bash step (#1851). The daemon-child path already does this via
/// [`inject_plan_daemon_session_into_startup_env`]; the foreground path had no
/// equivalent, leaving bash steps without CSA_SESSION_DIR.
///
/// Three cases, cheapest first:
/// - both id and dir already present (nested invocation whose parent exported
///   the full contract): reuse untouched, mint nothing.
/// - id present but dir missing: derive the canonical dir from the id without
///   creating a new session.
/// - neither present (top-level `--foreground` / `--resume`): mint a fresh
///   placeholder plan session and report its id for later retirement.
fn establish_foreground_plan_session(
    startup_env: &StartupSubtreeEnv,
    project_root: &Path,
    description: &str,
) -> Result<ForegroundPlanSession> {
    if startup_env.session_id().is_some() && startup_env.session_dir().is_some() {
        return Ok(ForegroundPlanSession {
            startup_env: startup_env.clone(),
            minted_session_id: None,
        });
    }

    if let Some(session_id) = startup_env.session_id() {
        let session_dir = csa_session::get_session_dir(project_root, session_id)?;
        let startup_env = startup_env
            .clone()
            .with_current_session(session_id, session_dir.display().to_string());
        return Ok(ForegroundPlanSession {
            startup_env,
            minted_session_id: None,
        });
    }

    let session_id = csa_session::new_session_id();
    let session_root = csa_session::get_session_root(project_root)?;
    let session_dir = session_root.join("sessions").join(&session_id);
    persist_placeholder_plan_session(project_root, &session_dir, &session_id, description)?;
    let startup_env = startup_env
        .clone()
        .with_current_session(&session_id, session_dir.display().to_string());
    Ok(ForegroundPlanSession {
        startup_env,
        minted_session_id: Some(session_id),
    })
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

/// Input snapshot for [`decide_needs_foreground`]. Bundling these into a
/// struct keeps the decision pure (no env reads, no globals) so the gating
/// logic can be tested in isolation without `unsafe { set_var }` plumbing.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ForegroundDecisionInput {
    pub foreground: bool,
    pub dry_run: bool,
    pub chunked: bool,
    pub has_resume: bool,
    pub current_depth: u32,
    pub nested_env: bool,
}

/// Decide whether `csa plan run` must execute foreground (block on the
/// inline workflow run) or may daemonize (default for the top-level user
/// invocation).
///
/// Forces foreground when:
/// - `--foreground` was explicitly requested by the user, or
/// - `--dry-run`/`--chunked`/`--resume` need synchronous stdout (printed
///   plan, JSON status, awaiting-user prompts), or
/// - nested invocation detected (`current_depth > 0` or
///   `CSA_*_SESSION_ID` env present). Nested callers — workflow.toml bash
///   steps, post-PR-create hooks, anything spawned from inside another
///   csa session — depend on the synchronous exit-code contract; e.g.
///   `dev2merge` step 14 (`if csa plan run patterns/pr-bot/workflow.toml;
///   then ...`) and `MKTD_OUTPUT="$(... csa plan run patterns/mktd/...)"`.
///   Daemonizing those silently bypasses the gate. See #1130 PR-1
///   cumulative review F1.
pub(crate) fn decide_needs_foreground(input: ForegroundDecisionInput) -> bool {
    let nested_invocation = input.current_depth > 0 || input.nested_env;
    nested_invocation || input.foreground || input.dry_run || input.chunked || input.has_resume
}

/// True when the current process appears to be running inside another CSA
/// session (a nested invocation). Used to gate the default daemon flip so
/// only the top-level user invocation daemonizes; nested callers preserve
/// the synchronous exit-code contract their if/$(...)/timeout patterns
/// depend on.
///
/// Checks several markers, any of which indicates "we are inside CSA":
/// - startup `CSA_SESSION_ID` — frozen before startup scrub, set by
///   `handle_plan_run_daemon_child` and the ACP transport for genealogy
///   attribution
/// - `CSA_DAEMON_SESSION_ID` — set by every daemon-child path
/// - `CSA_PARENT_SESSION_ID` — set when an executor spawns a sub-csa
fn nested_session_env_present(startup_env: &StartupSubtreeEnv) -> bool {
    if startup_env.session_id().is_some() {
        return true;
    }
    const MARKERS: &[&str] = &["CSA_DAEMON_SESSION_ID", "CSA_PARENT_SESSION_ID"];
    MARKERS.iter().any(|key| {
        std::env::var(key)
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
    })
}

#[path = "plan_cmd_daemon_forwarding.rs"]
mod forwarding;
pub(crate) use forwarding::{build_forwarded_plan_args, forwarded_args_with_feature_input};

#[cfg(test)]
#[path = "plan_cmd_daemon_tests.rs"]
mod tests;
