use std::path::Path;

use anyhow::Result;
use chrono::Utc;
use csa_session::{MetaSessionState, PhaseEvent, save_session};
use tracing::warn;

use super::{PLAN_TASK_TYPE, PlanRunArgs};

pub(super) fn describe_plan_run(args: &PlanRunArgs) -> String {
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

pub(super) fn persist_placeholder_plan_session(
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

pub(super) fn mark_session_as_plan(
    project_root: &Path,
    session_id: &str,
    description: &str,
) -> Result<()> {
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

pub(super) fn retire_plan_session(project_root: &Path, session_id: &str) -> Result<()> {
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
