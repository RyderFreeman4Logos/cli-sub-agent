use std::fs;
use std::path::Path;
use std::str::FromStr;

use crate::cli::ReviewArgs;
use anyhow::{Context, Result};
use csa_core::types::ReviewDecision;
use csa_session::ReviewVerdictArtifact;
use csa_session::state::{MetaSessionState, ReviewSessionMeta};
use tracing::debug;

const REQUIRED_FULL_DIFF_SCOPE: &str = "range:main...HEAD";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReviewVerdictMatch {
    pub session_id: String,
    pub scope: String,
    pub head_sha: String,
}

pub(crate) fn handle_check_verdict(project_root: &Path, args: &ReviewArgs) -> Result<i32> {
    let backend = csa_session::create_vcs_backend(project_root);
    let identity = backend
        .identity(project_root)
        .map_err(|error| anyhow::anyhow!("failed to resolve current VCS identity: {error}"))?;
    let branch = identity
        .ref_name
        .filter(|name| !name.trim().is_empty())
        .context("failed to resolve current branch for review verdict check")?;
    let head_sha = identity
        .commit_id
        .filter(|sha| !sha.trim().is_empty())
        .context("failed to resolve current HEAD SHA for review verdict check")?;
    let required_scope = required_check_verdict_scope(args);

    let diff_fingerprint = super::execute::compute_diff_fingerprint(project_root, &required_scope);

    match check_review_verdict_for_target(
        project_root,
        &branch,
        &head_sha,
        &required_scope,
        diff_fingerprint.as_deref(),
    ) {
        Ok(Some(found)) => {
            println!(
                "Review verdict check passed: session {} has PASS/CLEAN for {} at {} ({})",
                found.session_id,
                branch,
                short_sha(&found.head_sha),
                found.scope
            );
            Ok(0)
        }
        Ok(None) => {
            println!(
                "Review verdict check failed: no PASS/CLEAN full-diff review ({}) found for {} at {}.",
                required_scope,
                branch,
                short_sha(&head_sha)
            );
            Ok(1)
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn check_review_verdict_for_target(
    project_root: &Path,
    branch: &str,
    head_sha: &str,
    required_scope: &str,
    expected_diff_fingerprint: Option<&str>,
) -> Result<Option<ReviewVerdictMatch>> {
    let session_root = csa_session::get_session_root(project_root).with_context(|| {
        format!(
            "failed to resolve CSA session root for {}",
            project_root.display()
        )
    })?;
    let sessions = csa_session::list_sessions_from_root_readonly(&session_root)
        .with_context(|| format!("failed to list CSA sessions for {}", session_root.display()))?;
    debug!(
        project_root = %project_root.display(),
        branch,
        head_sha,
        ?expected_diff_fingerprint,
        session_count = sessions.len(),
        "Checking review verdict sessions"
    );

    let mut candidates = Vec::new();
    for session in sessions {
        let session_branch = session_branch(&session);
        debug!(
            session_id = %session.meta_session_id,
            ?session_branch,
            expected_branch = branch,
            "Considering review verdict session"
        );
        if !session_matches_branch(&session, branch) {
            debug!(
                session_id = %session.meta_session_id,
                ?session_branch,
                expected_branch = branch,
                "Skipping review verdict session: branch mismatch"
            );
            continue;
        }
        let session_dir = session_root.join("sessions").join(&session.meta_session_id);
        let Some(meta) = read_review_meta(&session_dir)? else {
            debug!(
                session_id = %session.meta_session_id,
                session_dir = %session_dir.display(),
                "Skipping review verdict session: missing review_meta.json"
            );
            continue;
        };
        if meta.head_sha != head_sha || meta.scope != required_scope {
            debug!(
                session_id = %session.meta_session_id,
                meta_head_sha = %meta.head_sha,
                expected_head_sha = head_sha,
                meta_scope = %meta.scope,
                expected_scope = required_scope,
                "Skipping review verdict session: head SHA or scope mismatch"
            );
            continue;
        }
        if !diff_fingerprint_matches(&meta, expected_diff_fingerprint) {
            debug!(
                session_id = %session.meta_session_id,
                meta_diff_fingerprint = ?meta.diff_fingerprint,
                ?expected_diff_fingerprint,
                "Skipping review verdict session: diff fingerprint mismatch"
            );
            continue;
        }
        let is_pass = review_meta_or_artifact_is_pass(&session_dir, &meta)?;
        if !is_pass {
            debug!(
                session_id = %session.meta_session_id,
                decision = %meta.decision,
                verdict = %meta.verdict,
                timestamp = %meta.timestamp,
                "Found matching review verdict session: non-pass candidate"
            );
        }
        debug!(
            session_id = %session.meta_session_id,
            scope = %meta.scope,
            head_sha = %meta.head_sha,
            timestamp = %meta.timestamp,
            is_pass,
            "Found matching review verdict candidate"
        );
        candidates.push(ReviewVerdictCandidate {
            session_id: meta.session_id,
            scope: meta.scope,
            head_sha: meta.head_sha,
            timestamp: meta.timestamp,
            is_pass,
        });
    }

    let Some(latest) = candidates
        .into_iter()
        .max_by_key(|candidate| candidate.timestamp)
    else {
        return Ok(None);
    };
    if !latest.is_pass {
        debug!(
            session_id = %latest.session_id,
            timestamp = %latest.timestamp,
            "Latest matching review verdict is not PASS/CLEAN"
        );
        return Ok(None);
    }
    debug!(
        session_id = %latest.session_id,
        scope = %latest.scope,
        head_sha = %latest.head_sha,
        timestamp = %latest.timestamp,
        "Latest matching review verdict is PASS/CLEAN"
    );
    Ok(Some(ReviewVerdictMatch {
        session_id: latest.session_id,
        scope: latest.scope,
        head_sha: latest.head_sha,
    }))
}

struct ReviewVerdictCandidate {
    session_id: String,
    scope: String,
    head_sha: String,
    timestamp: chrono::DateTime<chrono::Utc>,
    is_pass: bool,
}

fn session_branch(session: &MetaSessionState) -> Option<&str> {
    session
        .vcs_identity
        .as_ref()
        .and_then(|identity| identity.ref_name.as_deref())
        .or(session.branch.as_deref())
}

fn session_matches_branch(session: &MetaSessionState, branch: &str) -> bool {
    session_branch(session) == Some(branch)
}

fn required_check_verdict_scope(args: &ReviewArgs) -> String {
    if let Some(range) = args.range.as_deref() {
        return format!("range:{range}");
    }
    if let Some(branch) = args.branch.as_deref() {
        return format!("base:{branch}");
    }
    REQUIRED_FULL_DIFF_SCOPE.to_string()
}

fn read_review_meta(session_dir: &Path) -> Result<Option<ReviewSessionMeta>> {
    let path = session_dir.join("review_meta.json");
    if !path.exists() {
        return Ok(None);
    }
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let meta = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(Some(meta))
}

fn diff_fingerprint_matches(
    meta: &ReviewSessionMeta,
    expected_diff_fingerprint: Option<&str>,
) -> bool {
    expected_diff_fingerprint
        .map(|expected| meta.diff_fingerprint.as_deref() == Some(expected))
        .unwrap_or(true)
}

fn review_meta_or_artifact_is_pass(session_dir: &Path, meta: &ReviewSessionMeta) -> Result<bool> {
    let meta_pass = review_meta_is_pass(meta);
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    if verdict_path.exists() {
        let raw = fs::read_to_string(&verdict_path)
            .with_context(|| format!("failed to read {}", verdict_path.display()))?;
        let artifact: ReviewVerdictArtifact = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse {}", verdict_path.display()))?;
        let artifact_pass = artifact.decision == ReviewDecision::Pass
            || verdict_token_is_pass(&artifact.verdict_legacy);
        debug!(
            session_id = %meta.session_id,
            meta_decision = %meta.decision,
            meta_verdict = %meta.verdict,
            meta_pass,
            artifact_decision = %artifact.decision,
            artifact_verdict = %artifact.verdict_legacy,
            artifact_pass,
            verdict_path = %verdict_path.display(),
            "Read review verdict artifact"
        );
        return Ok(artifact_pass);
    }

    debug!(
        session_id = %meta.session_id,
        meta_decision = %meta.decision,
        meta_verdict = %meta.verdict,
        meta_pass,
        "Using review_meta.json verdict"
    );
    Ok(meta_pass)
}

fn review_meta_is_pass(meta: &ReviewSessionMeta) -> bool {
    ReviewDecision::from_str(&meta.decision).is_ok_and(|decision| {
        decision == ReviewDecision::Pass || verdict_token_is_pass(&meta.verdict)
    }) || verdict_token_is_pass(&meta.verdict)
}

fn verdict_token_is_pass(verdict: &str) -> bool {
    matches!(
        verdict.trim().to_ascii_uppercase().as_str(),
        "PASS" | "CLEAN"
    )
}

fn short_sha(sha: &str) -> &str {
    sha.get(..sha.len().min(11)).unwrap_or(sha)
}

#[cfg(test)]
#[path = "review_cmd_check_verdict_tests.rs"]
mod tests;
