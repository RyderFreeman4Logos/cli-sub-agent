use std::io::ErrorKind;
use std::path::Path;

use chrono::{DateTime, Utc};
use csa_core::types::ReviewDecision;
use csa_session::{MetaSessionState, SessionArtifact, SessionResult};
use tracing::warn;

use super::completion::DaemonCompletionPacket;

const REVIEW_DAEMON_NO_RESULT_REASON: &str = "daemon_completion_before_result";
const REVIEW_VERDICT_ARTIFACT_PATH: &str = "output/review-verdict.json";
const REVIEW_DAEMON_TOOL_LAUNCH_METADATA_ABSENT: &str = "tool_launch_metadata_absent";

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

pub(crate) fn review_result_from_existing_artifacts(
    project_root: &Path,
    session_dir: &Path,
    session: &MetaSessionState,
    packet: &DaemonCompletionPacket,
    completed_at: DateTime<Utc>,
) -> Option<SessionResult> {
    if !is_review_daemon_session(session) {
        return None;
    }

    let verdict = recoverable_review_verdict(session_dir)?;
    let meta =
        recover_or_persist_review_meta_from_verdict(project_root, session_dir, session, &verdict)?;
    let exit_code = crate::verdict_exit_code::exit_code_from_review_decision(verdict.decision);
    let mut artifacts = crate::pipeline_post_exec::collect_fallback_result_artifacts(
        project_root,
        &session.meta_session_id,
    );
    ensure_review_verdict_artifact(&mut artifacts);

    let raw_process_exit_code = (packet.exit_code != exit_code).then_some(packet.exit_code);
    let warnings = raw_process_exit_code
        .map(|raw_exit_code| {
            vec![format!(
                "daemon completion exit code ({raw_exit_code}) arrived before result.toml; using existing review verdict artifacts"
            )]
        })
        .unwrap_or_default();

    Some(SessionResult {
        status: SessionResult::status_from_exit_code(exit_code),
        exit_code,
        summary: review_artifact_result_summary(&meta),
        tool: meta.tool.clone(),
        started_at: session.last_accessed,
        completed_at,
        events_count: 0,
        artifacts,
        raw_process_exit_code,
        warnings,
        ..Default::default()
    })
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

        if recoverable_review_verdict(session_dir).is_some() {
            warn!(
                result_path = %result_path.display(),
                rollback_cleanup = "existing_review_artifacts_preserved",
                "Skipped review daemon no-result diagnostic sidecars because recoverable review artifacts already exist"
            );
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
    if matches!(
        session.task_context.task_type.as_deref().map(str::trim),
        Some("review" | "reviewer_sub_session")
    ) {
        return true;
    }

    session
        .description
        .as_deref()
        .is_some_and(is_legacy_review_description)
}

fn is_legacy_review_description(description: &str) -> bool {
    let description = description.trim();
    description.eq_ignore_ascii_case("review")
        || description.eq_ignore_ascii_case("initializing daemon review")
        || has_ascii_prefix_ignore_case(description, "review:")
        || strip_multi_reviewer_scope(description).is_some()
        || has_ascii_prefix_ignore_case(description, "code-review:")
}

fn canonical_review_scope(session: &MetaSessionState) -> String {
    session
        .description
        .as_deref()
        .and_then(extract_review_scope)
        .or_else(|| {
            session
                .description
                .as_deref()
                .map(str::trim)
                .filter(|description| !description.is_empty())
        })
        .unwrap_or("unknown")
        .to_string()
}

fn extract_review_scope(description: &str) -> Option<&str> {
    let description = description.trim();
    strip_ascii_prefix_ignore_case(description, "review:")
        .or_else(|| strip_multi_reviewer_scope(description))
        .or_else(|| strip_ascii_prefix_ignore_case(description, "code-review:"))
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
}

fn strip_multi_reviewer_scope(description: &str) -> Option<&str> {
    let rest = strip_ascii_prefix_ignore_case(description, "review[")?;
    let close_bracket = rest.find(']')?;
    let reviewer_index = &rest[..close_bracket];
    if reviewer_index.is_empty() || !reviewer_index.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let after_bracket = rest.get(close_bracket + 1..)?.trim_start();
    after_bracket.strip_prefix(':').map(str::trim)
}

fn has_ascii_prefix_ignore_case(value: &str, prefix: &str) -> bool {
    strip_ascii_prefix_ignore_case(value, prefix).is_some()
}

fn strip_ascii_prefix_ignore_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    let candidate = value.get(..prefix.len())?;
    if candidate.eq_ignore_ascii_case(prefix) {
        value.get(prefix.len()..)
    } else {
        None
    }
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

fn read_review_meta(session_dir: &Path) -> Option<csa_session::ReviewSessionMeta> {
    std::fs::read_to_string(session_dir.join("review_meta.json"))
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
}

fn read_review_verdict(session_dir: &Path) -> Option<csa_session::ReviewVerdictArtifact> {
    std::fs::read_to_string(session_dir.join(REVIEW_VERDICT_ARTIFACT_PATH))
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
}

fn recoverable_review_verdict(session_dir: &Path) -> Option<csa_session::ReviewVerdictArtifact> {
    let verdict = read_review_verdict(session_dir)?;
    (!is_review_no_result_verdict(&verdict)).then_some(verdict)
}

fn is_review_no_result_meta(meta: &csa_session::ReviewSessionMeta) -> bool {
    meta.decision == ReviewDecision::Unavailable.as_str()
        && (meta.status_reason.as_deref() == Some(REVIEW_DAEMON_NO_RESULT_REASON)
            || matches!(
                meta.primary_failure.as_deref(),
                Some(REVIEW_DAEMON_NO_RESULT_REASON | REVIEW_DAEMON_TOOL_LAUNCH_METADATA_ABSENT)
            ))
}

fn is_review_no_result_verdict(verdict: &csa_session::ReviewVerdictArtifact) -> bool {
    verdict.decision == ReviewDecision::Unavailable
        && matches!(
            verdict.primary_failure.as_deref(),
            Some(REVIEW_DAEMON_NO_RESULT_REASON | REVIEW_DAEMON_TOOL_LAUNCH_METADATA_ABSENT)
        )
}

fn recover_or_persist_review_meta_from_verdict(
    project_root: &Path,
    session_dir: &Path,
    session: &MetaSessionState,
    verdict: &csa_session::ReviewVerdictArtifact,
) -> Option<csa_session::ReviewSessionMeta> {
    let existing_meta = read_review_meta(session_dir);
    if let Some(meta) = existing_meta.as_ref()
        && !is_review_no_result_meta(meta)
        && review_meta_matches_verdict(meta, verdict)
    {
        let Some(diff_fingerprint) = meta.diff_fingerprint.clone().or_else(|| {
            crate::review_cmd::compute_review_diff_fingerprint(project_root, &meta.scope)
        }) else {
            return existing_meta;
        };
        if meta.diff_fingerprint.as_deref() == Some(diff_fingerprint.as_str()) {
            return existing_meta;
        }

        let mut recovered_meta = meta.clone();
        recovered_meta.diff_fingerprint = Some(diff_fingerprint);
        persist_recovered_review_meta(session_dir, session, &recovered_meta);
        return Some(recovered_meta);
    }

    let meta = review_meta_from_verdict(
        project_root,
        session_dir,
        session,
        verdict,
        existing_meta.as_ref(),
    );
    persist_recovered_review_meta(session_dir, session, &meta);
    Some(meta)
}

fn persist_recovered_review_meta(
    session_dir: &Path,
    session: &MetaSessionState,
    meta: &csa_session::ReviewSessionMeta,
) {
    if let Err(err) = csa_session::write_review_meta(session_dir, meta) {
        warn!(
            session_id = %session.meta_session_id,
            path = %session_dir.join("review_meta.json").display(),
            error = %err,
            "Failed to persist recovered review metadata from existing review verdict artifact; using in-memory metadata"
        );
    }
}

fn review_meta_matches_verdict(
    meta: &csa_session::ReviewSessionMeta,
    verdict: &csa_session::ReviewVerdictArtifact,
) -> bool {
    meta.decision == verdict.decision.as_str()
        && meta.verdict == verdict.verdict_legacy
        && meta.exit_code
            == crate::verdict_exit_code::exit_code_from_review_decision(verdict.decision)
}

fn review_meta_from_verdict(
    project_root: &Path,
    session_dir: &Path,
    session: &MetaSessionState,
    verdict: &csa_session::ReviewVerdictArtifact,
    previous_meta: Option<&csa_session::ReviewSessionMeta>,
) -> csa_session::ReviewSessionMeta {
    let decision = verdict.decision;
    let fallback_tool = review_no_result_tool(session_dir, session);
    let scope = previous_meta
        .map(|meta| meta.scope.clone())
        .filter(|scope| !scope.trim().is_empty())
        .unwrap_or_else(|| canonical_review_scope(session));
    let diff_fingerprint = previous_meta
        .and_then(|meta| meta.diff_fingerprint.clone())
        .or_else(|| crate::review_cmd::compute_review_diff_fingerprint(project_root, &scope));
    csa_session::ReviewSessionMeta {
        session_id: session.meta_session_id.clone(),
        head_sha: previous_meta
            .map(|meta| meta.head_sha.clone())
            .filter(|head| !head.trim().is_empty())
            .or_else(|| session.git_head_at_creation.clone())
            .unwrap_or_default(),
        decision: decision.as_str().to_string(),
        verdict: verdict.verdict_legacy.clone(),
        review_mode: verdict
            .review_mode
            .clone()
            .or_else(|| previous_meta.and_then(|meta| meta.review_mode.clone())),
        status_reason: None,
        routed_to: verdict.routed_to.clone(),
        primary_failure: verdict.primary_failure.clone(),
        failure_reason: verdict.failure_reason.clone(),
        tool: previous_meta
            .map(|meta| meta.tool.clone())
            .filter(|tool| !tool.trim().is_empty() && tool != "unknown")
            .unwrap_or(fallback_tool),
        scope,
        exit_code: crate::verdict_exit_code::exit_code_from_review_decision(decision),
        fix_attempted: previous_meta.is_some_and(|meta| meta.fix_attempted),
        fix_rounds: previous_meta.map_or(0, |meta| meta.fix_rounds),
        review_iterations: previous_meta.map_or(1, |meta| meta.review_iterations),
        timestamp: Utc::now(),
        diff_fingerprint,
        fix_convergence: previous_meta.and_then(|meta| meta.fix_convergence.clone()),
    }
}

fn review_artifact_result_summary(meta: &csa_session::ReviewSessionMeta) -> String {
    let verdict = meta.verdict.trim();
    let decision = meta.decision.trim();
    if verdict.is_empty() {
        format!(
            "review completed with decision {decision}; recovered existing review artifacts before daemon completion result.toml was published"
        )
    } else {
        format!(
            "review completed with verdict {verdict} ({decision}); recovered existing review artifacts before daemon completion result.toml was published"
        )
    }
}

fn ensure_review_verdict_artifact(artifacts: &mut Vec<SessionArtifact>) {
    if artifacts
        .iter()
        .any(|artifact| artifact.path == REVIEW_VERDICT_ARTIFACT_PATH)
    {
        return;
    }
    artifacts.push(csa_session::observed_session_artifact(
        REVIEW_VERDICT_ARTIFACT_PATH,
    ));
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
        scope: canonical_review_scope(session),
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
