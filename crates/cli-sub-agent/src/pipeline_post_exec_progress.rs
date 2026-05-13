//! Git-based progress classification for successful `csa run` sessions.

use std::path::Path;
use std::process::Command;

use tracing::warn;

use csa_session::{MetaSessionState, SessionResult};

const NO_PROGRESS_STATUS: &str = "no_progress";
const NO_PROGRESS_SUMMARY_PREFIX: &str = "tool exited successfully but produced no changes";

pub(crate) fn maybe_mark_no_progress_session(
    project_root: &Path,
    session: &mut MetaSessionState,
    result: &mut csa_process::ExecutionResult,
    session_result: &mut SessionResult,
) -> anyhow::Result<()> {
    let Some(start_head) = session
        .change_id
        .as_deref()
        .or(session.git_head_at_creation.as_deref())
        .map(str::to_string)
    else {
        anyhow::bail!("session has no start HEAD/change_id");
    };

    let progress = detect_git_progress_since_start(project_root, &start_head)?;
    if progress.has_progress() {
        return Ok(());
    }

    let original_summary = session_result.summary.clone();
    let diagnostic = format!(
        "{NO_PROGRESS_SUMMARY_PREFIX}: no diff, commits, or uncommitted status since start HEAD {}. Original: {}",
        abbreviate_revision(&start_head),
        original_summary,
    );
    warn!(
        session = %session.meta_session_id,
        start_head = %start_head,
        "Run session exited 0 without commits or file changes; marking result status no_progress"
    );
    session_result.status = NO_PROGRESS_STATUS.to_string();
    session_result.summary = diagnostic.clone();
    result.summary = diagnostic.clone();
    session.termination_reason = Some(NO_PROGRESS_STATUS.to_string());
    if let Some(tool_state) = session.tools.get_mut(session_result.tool.as_str()) {
        tool_state.last_action_summary = diagnostic;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitProgress {
    diff_stat: String,
    commit_log: String,
    porcelain: String,
}

impl GitProgress {
    fn has_progress(&self) -> bool {
        !self.diff_stat.trim().is_empty()
            || !self.commit_log.trim().is_empty()
            || !self.porcelain.trim().is_empty()
    }
}

fn detect_git_progress_since_start(
    project_root: &Path,
    start_head: &str,
) -> anyhow::Result<GitProgress> {
    let diff_stat = run_git_stdout(project_root, &["diff", "--stat", start_head, "--"])?;
    let rev_range = format!("{start_head}..HEAD");
    let commit_log = run_git_stdout(project_root, &["log", "--oneline", &rev_range])?;
    let porcelain = run_git_stdout(project_root, &["status", "--porcelain"])?;
    Ok(GitProgress {
        diff_stat,
        commit_log,
        porcelain,
    })
}

fn run_git_stdout(project_root: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(project_root)
        .output()
        .map_err(|err| anyhow::anyhow!("failed to run git {args:?}: {err}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "git {:?} failed with status {:?}: {}",
            args,
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn abbreviate_revision(revision: &str) -> &str {
    revision.get(..12).unwrap_or(revision)
}
