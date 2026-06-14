use std::io::ErrorKind;
use std::path::Path;

use csa_core::types::ReviewDecision;
use csa_session::{MetaSessionState, SessionArtifact};
use tracing::warn;

use super::completion::DaemonCompletionPacket;

const REVIEW_DAEMON_NO_RESULT_REASON: &str = "daemon_completion_before_result";
const REVIEW_VERDICT_ARTIFACT_PATH: &str = "output/review-verdict.json";

pub(crate) fn has_review_no_result_diagnostic(session: &MetaSessionState) -> bool {
    is_review_daemon_session(session)
}

pub(crate) fn append_review_no_result_diagnostic_artifacts(
    artifacts: &mut Vec<SessionArtifact>,
    session: &MetaSessionState,
) {
    if !has_review_no_result_diagnostic(session) {
        return;
    }
    if artifacts
        .iter()
        .any(|artifact| artifact.path == REVIEW_VERDICT_ARTIFACT_PATH)
    {
        return;
    }
    artifacts.push(SessionArtifact::new(REVIEW_VERDICT_ARTIFACT_PATH));
}

impl DaemonCompletionPacket {
    pub(crate) fn persist_review_diag(
        &self,
        result_path: &Path,
        expected_contents: &[u8],
        session_dir: &Path,
        session: &MetaSessionState,
    ) {
        if !has_review_no_result_diagnostic(session) {
            return;
        }

        match std::fs::read(result_path) {
            Ok(current_contents) if current_contents == expected_contents => {
                persist_review_no_result_diagnostic(session_dir, session, self)
            }
            Ok(_) => warn!(
                result_path = %result_path.display(),
                rollback_cleanup = "late_real_result_preserved",
                "Skipped review daemon no-result diagnostic sidecars because result.toml changed after synthetic daemon completion publication"
            ),
            Err(err) if err.kind() == ErrorKind::NotFound => warn!(
                result_path = %result_path.display(),
                rollback_cleanup = "result_missing",
                "Skipped review daemon no-result diagnostic sidecars because synthetic daemon completion result.toml disappeared before sidecar publication"
            ),
            Err(err) => warn!(
                result_path = %result_path.display(),
                rollback_cleanup = "read_failed",
                error = %err,
                "Skipped review daemon no-result diagnostic sidecars because synthetic daemon completion result.toml could not be verified"
            ),
        }
    }
}

pub(crate) fn persist_review_no_result_diagnostic(
    session_dir: &Path,
    session: &MetaSessionState,
    packet: &DaemonCompletionPacket,
) {
    if !is_review_daemon_session(session) {
        return;
    }

    let tool = review_no_result_tool(session_dir, session);
    let diagnostic = review_no_result_summary(packet, &tool);
    persist_review_no_result_meta(session_dir, session, &tool, &diagnostic);
    persist_review_no_result_verdict(session_dir, session, &tool, &diagnostic);
}

fn is_review_daemon_session(session: &MetaSessionState) -> bool {
    session
        .description
        .as_deref()
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .is_some_and(|description| {
            description == "review"
                || description.starts_with("review:")
                || description == "initializing daemon review"
        })
}

fn review_no_result_summary(packet: &DaemonCompletionPacket, tool: &str) -> String {
    let tool_note = if tool == "unknown" {
        "; no tool launch metadata was recorded"
    } else {
        ""
    };
    format!(
        "review daemon completion recorded status={} exit_code={}{} before result.toml was written{tool_note}",
        packet.status,
        packet.exit_code,
        packet
            .reason
            .as_deref()
            .map(|reason| format!(" reason={reason}"))
            .unwrap_or_default()
    )
}

fn review_no_result_tool(session_dir: &Path, session: &MetaSessionState) -> String {
    session
        .tools
        .iter()
        .max_by_key(|(_, state)| state.updated_at)
        .map(|(tool, _)| tool.clone())
        .or_else(|| {
            let metadata_path = session_dir.join(csa_session::metadata::METADATA_FILE_NAME);
            std::fs::read_to_string(metadata_path)
                .ok()
                .and_then(|content| {
                    toml::from_str::<csa_session::metadata::SessionMetadata>(&content).ok()
                })
                .map(|metadata| metadata.tool)
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn persist_review_no_result_meta(
    session_dir: &Path,
    session: &MetaSessionState,
    tool: &str,
    diagnostic: &str,
) {
    let path = session_dir.join("review_meta.json");
    if path.is_file() {
        return;
    }

    let meta = csa_session::ReviewSessionMeta {
        session_id: session.meta_session_id.clone(),
        head_sha: session.git_head_at_creation.clone().unwrap_or_default(),
        decision: ReviewDecision::Unavailable.as_str().to_string(),
        verdict: "UNAVAILABLE".to_string(),
        review_mode: None,
        status_reason: Some(REVIEW_DAEMON_NO_RESULT_REASON.to_string()),
        routed_to: None,
        primary_failure: (tool == "unknown").then(|| "tool_launch_metadata_absent".to_string()),
        failure_reason: Some(diagnostic.to_string()),
        tool: tool.to_string(),
        scope: session
            .description
            .as_deref()
            .unwrap_or("unknown")
            .to_string(),
        exit_code: crate::verdict_exit_code::exit_code_from_review_decision(
            ReviewDecision::Unavailable,
        ),
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations: 1,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
        fix_convergence: None,
    };

    if let Err(err) = csa_session::write_review_meta(session_dir, &meta) {
        warn!(
            session_id = %session.meta_session_id,
            path = %path.display(),
            error = %err,
            "Failed to persist review daemon no-result metadata"
        );
    }
}

fn persist_review_no_result_verdict(
    session_dir: &Path,
    session: &MetaSessionState,
    tool: &str,
    diagnostic: &str,
) {
    let path = session_dir.join("output").join("review-verdict.json");
    if path.is_file() {
        return;
    }

    let mut artifact = csa_session::ReviewVerdictArtifact::from_parts(
        session.meta_session_id.clone(),
        ReviewDecision::Unavailable,
        "UNAVAILABLE",
        &[],
        Vec::new(),
    );
    artifact.primary_failure = Some(if tool == "unknown" {
        "tool_launch_metadata_absent".to_string()
    } else {
        REVIEW_DAEMON_NO_RESULT_REASON.to_string()
    });
    artifact.failure_reason = Some(diagnostic.to_string());

    if let Err(err) = csa_session::write_review_verdict(session_dir, &artifact) {
        warn!(
            session_id = %session.meta_session_id,
            path = %path.display(),
            error = %err,
            "Failed to persist review daemon no-result verdict artifact"
        );
    }
}
