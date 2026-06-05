use std::path::Path;
use std::process::Command;

use csa_core::types::ReviewDecision;
use csa_session::state::ReviewSessionMeta;
use tracing::warn;

use super::convergence::FixTerminalOutcome;

const FIX_LOOP_NOOP_REASON: &str = "head_unchanged_worktree_clean";
const FIX_LOOP_NOOP_FAILURE_PREFIX: &str = "fix_loop_noop";

#[derive(Debug)]
pub(super) struct FixNoOpProbe {
    pub(super) initial_head: Option<String>,
}

impl FixNoOpProbe {
    pub(super) fn capture(project_root: &Path) -> Self {
        Self {
            initial_head: csa_session::detect_git_head(project_root),
        }
    }
}

pub(super) fn apply_fix_loop_noop_signal(
    project_root: &Path,
    mut final_meta: ReviewSessionMeta,
    outcome: &FixTerminalOutcome,
    noop_probe: Option<&FixNoOpProbe>,
) -> ReviewSessionMeta {
    let Some(reason) = detect_fix_loop_noop(project_root, outcome, noop_probe) else {
        return final_meta;
    };
    let failure_reason = fix_loop_noop_failure_reason(reason);
    final_meta.failure_reason = Some(failure_reason.clone());
    if let Some(convergence) = final_meta.fix_convergence.as_mut() {
        convergence.terminal_reason = failure_reason;
    }
    persist_fix_loop_noop_result_signal(project_root, &final_meta.session_id, reason);
    final_meta
}

fn detect_fix_loop_noop(
    project_root: &Path,
    outcome: &FixTerminalOutcome,
    noop_probe: Option<&FixNoOpProbe>,
) -> Option<&'static str> {
    if outcome.reached_genuine_clean_convergence()
        || outcome.post_consistency_decision != ReviewDecision::Fail
    {
        return None;
    }
    let initial_head = noop_probe?.initial_head.as_deref()?.trim();
    if initial_head.is_empty() {
        return None;
    }
    let current_head = csa_session::detect_git_head(project_root)?;
    if current_head.trim() != initial_head {
        return None;
    }
    git_worktree_is_clean(project_root).then_some(FIX_LOOP_NOOP_REASON)
}

fn git_worktree_is_clean(project_root: &Path) -> bool {
    let Ok(output) = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["status", "--porcelain=v1", "--untracked-files=normal"])
        .output()
    else {
        return false;
    };
    output.status.success() && output.stdout.is_empty()
}

fn fix_loop_noop_failure_reason(reason: &str) -> String {
    format!("{FIX_LOOP_NOOP_FAILURE_PREFIX}:{reason}")
}

pub(super) fn is_fix_loop_noop_failure_reason(reason: &str) -> bool {
    reason
        .strip_prefix(FIX_LOOP_NOOP_FAILURE_PREFIX)
        .is_some_and(|suffix| suffix.starts_with(':'))
}

fn fix_loop_noop_message(reason: &str) -> String {
    format!("fix loop did not engage: {reason}")
}

fn persist_fix_loop_noop_result_signal(project_root: &Path, session_id: &str, reason: &str) {
    let message = fix_loop_noop_message(reason);
    let mut result = match csa_session::load_result(project_root, session_id) {
        Ok(Some(result)) => result,
        Ok(None) => return,
        Err(error) => {
            warn!(
                session_id,
                error = %error,
                "Failed to load result.toml while recording fix-loop no-op signal"
            );
            return;
        }
    };
    result.summary = message.clone();
    if !result.warnings.iter().any(|warning| warning == &message) {
        result.warnings.push(message);
    }
    if let Err(error) = csa_session::save_result(project_root, session_id, &result) {
        warn!(
            session_id,
            error = %error,
            "Failed to persist result.toml fix-loop no-op signal"
        );
    }
}
