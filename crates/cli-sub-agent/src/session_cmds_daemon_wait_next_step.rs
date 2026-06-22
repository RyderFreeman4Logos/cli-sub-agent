use std::fs;
use std::path::Path;

use anyhow::Result;
use csa_session::{ReviewVerdictArtifact, state::ReviewSessionMeta};

use super::super::{POST_REVIEW_PR_BOT_CMD, UnpushedCommitsRecoveryPacket};

pub(crate) fn synthesized_wait_next_step(session_dir: &Path) -> Result<Option<String>> {
    let stdout_path = session_dir.join("stdout.log");
    if let Ok(stdout) = fs::read_to_string(&stdout_path)
        && csa_hooks::parse_next_step_directive(&stdout).is_some()
    {
        return Ok(None);
    }
    if crate::session_fix_finding_recovery::suppresses_required_push_next_step(session_dir) {
        return Ok(None);
    }

    let unpushed_commits_path = session_dir.join("output").join("unpushed_commits.json");
    if unpushed_commits_path.is_file() {
        match fs::read_to_string(&unpushed_commits_path) {
            Ok(contents) => {
                match serde_json::from_str::<UnpushedCommitsRecoveryPacket>(&contents) {
                    Ok(recovery) if !recovery.recovery_command.trim().is_empty() => {
                        return Ok(Some(csa_hooks::format_next_step_directive(
                            &recovery.recovery_command,
                            true,
                        )));
                    }
                    Ok(_) => {}
                    Err(err) => {
                        tracing::warn!(
                            sidecar_path = %unpushed_commits_path.display(),
                            error = %err,
                            "Ignoring malformed unpushed commit recovery sidecar while synthesizing wait next-step"
                        );
                    }
                }
            }
            Err(err) => {
                tracing::debug!(
                    sidecar_path = %unpushed_commits_path.display(),
                    error = %err,
                    "Ignoring unreadable unpushed commit recovery sidecar while synthesizing wait next-step"
                );
            }
        }
    }

    let review_meta_path = session_dir.join("review_meta.json");
    if !review_meta_path.is_file() {
        return Ok(None);
    }

    let review_meta: ReviewSessionMeta =
        serde_json::from_str(&fs::read_to_string(review_meta_path)?)?;
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let Ok(verdict_raw) = fs::read_to_string(&verdict_path) else {
        return Ok(None);
    };
    let Ok(verdict) = serde_json::from_str::<ReviewVerdictArtifact>(&verdict_raw) else {
        return Ok(None);
    };
    if !review_meta.accepts_clean_review_verdict(verdict.decision) {
        return Ok(None);
    }
    if !(review_meta.scope.starts_with("base:") || review_meta.scope.starts_with("range:")) {
        return Ok(None);
    }

    Ok(Some(csa_hooks::format_next_step_directive(
        POST_REVIEW_PR_BOT_CMD,
        true,
    )))
}
