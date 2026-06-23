use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::cli::ReviewArgs;
use anyhow::{Context, Result};
use csa_session::state::{MetaSessionState, ReviewSessionMeta};
use csa_session::{ReviewVerdictArtifact, Severity};
use tracing::debug;

const REQUIRED_FULL_DIFF_SCOPE: &str = "range:main...HEAD";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReviewVerdictMatch {
    pub session_id: String,
    pub scope: String,
    pub head_sha: String,
    pub severity_counts: BTreeMap<Severity, u32>,
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
    let required_mode = required_check_verdict_mode(args);

    let diff_fingerprint = super::execute::compute_diff_fingerprint(project_root, &required_scope);

    let verdict_match = if let Some(session) = args.session.as_deref() {
        check_review_verdict_for_session(
            project_root,
            session,
            &branch,
            &head_sha,
            &required_scope,
            diff_fingerprint.as_deref(),
            required_mode.as_deref(),
        )
    } else {
        check_review_verdict_for_target(
            project_root,
            &branch,
            &head_sha,
            &required_scope,
            diff_fingerprint.as_deref(),
            required_mode.as_deref(),
        )
    };

    match verdict_match {
        Ok(Some(found)) => {
            let message = format_review_verdict_pass_message(&found, &branch);
            println!("{message}");
            Ok(0)
        }
        Ok(None) => {
            let mode_requirement = required_mode
                .as_deref()
                .map(|mode| format!(" in review mode '{mode}'"))
                .unwrap_or_default();
            println!(
                "Review verdict check failed: no PASS/CLEAN full-diff review ({}){} found for {} at {}.",
                required_scope,
                mode_requirement,
                branch,
                short_sha(&head_sha)
            );
            Ok(1)
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn check_review_verdict_for_session(
    project_root: &Path,
    session_prefix: &str,
    branch: &str,
    head_sha: &str,
    required_scope: &str,
    expected_diff_fingerprint: Option<&str>,
    required_mode: Option<&str>,
) -> Result<Option<ReviewVerdictMatch>> {
    let resolved =
        crate::session_cmds::resolve_session_prefix_with_fallback(project_root, session_prefix)?;
    let session_dir = resolved.sessions_dir.join(&resolved.session_id);
    let session = read_session_state(&session_dir)?;
    debug!(
        project_root = %project_root.display(),
        session_id = %resolved.session_id,
        branch,
        head_sha,
        required_scope,
        ?expected_diff_fingerprint,
        "Checking explicit review verdict session"
    );

    if !session_matches_branch(&session, branch) {
        debug!(
            session_id = %resolved.session_id,
            session_branch = ?session_branch(&session),
            expected_branch = branch,
            "Explicit review verdict session did not match branch"
        );
        return Ok(None);
    }

    let Some(meta) =
        read_review_meta_after_recovery(project_root, &resolved.session_id, &session_dir)?
    else {
        debug!(
            session_id = %resolved.session_id,
            session_dir = %session_dir.display(),
            "Explicit review verdict session is missing review_meta.json"
        );
        return Ok(None);
    };

    if meta.head_sha != head_sha || meta.scope != required_scope {
        debug!(
            session_id = %resolved.session_id,
            meta_head_sha = %meta.head_sha,
            expected_head_sha = head_sha,
            meta_scope = %meta.scope,
            expected_scope = required_scope,
            "Explicit review verdict session did not match head SHA or scope"
        );
        return Ok(None);
    }

    if !review_mode_matches(meta.review_mode.as_deref(), required_mode) {
        debug!(
            session_id = %resolved.session_id,
            meta_review_mode = ?meta.review_mode,
            ?required_mode,
            "Explicit review verdict session did not match required review mode"
        );
        return Ok(None);
    }

    if !diff_fingerprint_matches(&meta, expected_diff_fingerprint) {
        debug!(
            session_id = %resolved.session_id,
            meta_diff_fingerprint = ?meta.diff_fingerprint,
            ?expected_diff_fingerprint,
            "Explicit review verdict session did not match diff fingerprint"
        );
        return Ok(None);
    }

    let pass_status = review_session_acceptance_status(&session_dir, &meta)?;
    if !pass_status.is_pass {
        debug!(
            session_id = %resolved.session_id,
            decision = %meta.decision,
            verdict = %meta.verdict,
            "Explicit review verdict session is not PASS/CLEAN"
        );
        return Ok(None);
    }

    Ok(Some(ReviewVerdictMatch {
        session_id: meta.session_id,
        scope: meta.scope,
        head_sha: meta.head_sha,
        severity_counts: pass_status.severity_counts,
    }))
}

pub(crate) fn check_review_verdict_for_target(
    project_root: &Path,
    branch: &str,
    head_sha: &str,
    required_scope: &str,
    expected_diff_fingerprint: Option<&str>,
    required_mode: Option<&str>,
) -> Result<Option<ReviewVerdictMatch>> {
    let marker_candidate = check_review_verdict_marker(
        project_root,
        branch,
        head_sha,
        required_scope,
        expected_diff_fingerprint,
        required_mode,
    )?;

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
    if let Some(candidate) = marker_candidate {
        candidates.push(candidate);
    }
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
        let Some(meta) =
            read_review_meta_after_recovery(project_root, &session.meta_session_id, &session_dir)?
        else {
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
        if !review_mode_matches(meta.review_mode.as_deref(), required_mode) {
            debug!(
                session_id = %session.meta_session_id,
                meta_review_mode = ?meta.review_mode,
                ?required_mode,
                "Skipping review verdict session: review mode mismatch"
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
        let pass_status = review_session_acceptance_status(&session_dir, &meta)?;
        if !pass_status.is_pass {
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
            is_pass = pass_status.is_pass,
            "Found matching review verdict candidate"
        );
        candidates.push(ReviewVerdictCandidate {
            session_id: meta.session_id,
            scope: meta.scope,
            head_sha: meta.head_sha,
            timestamp: meta.timestamp,
            is_pass: pass_status.is_pass,
            severity_counts: pass_status.severity_counts,
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
        severity_counts: latest.severity_counts,
    }))
}

fn check_review_verdict_marker(
    project_root: &Path,
    branch: &str,
    head_sha: &str,
    required_scope: &str,
    expected_diff_fingerprint: Option<&str>,
    required_mode: Option<&str>,
) -> Result<Option<ReviewVerdictCandidate>> {
    let Some(marker) = crate::review_gate::read_review_gate_marker(project_root, branch, head_sha)
    else {
        return Ok(None);
    };
    debug!(
        session_id = %marker.session_id,
        marker_branch = %marker.branch,
        expected_branch = branch,
        marker_head_sha = %marker.head_sha,
        expected_head_sha = head_sha,
        marker_scope = %marker.scope,
        expected_scope = required_scope,
        marker_verdict = %marker.verdict,
        marker_timestamp = %marker.timestamp,
        "Checking review verdict marker"
    );

    if marker.branch != branch
        || marker.head_sha != head_sha
        || marker.scope != required_scope
        || !verdict_token_is_pass(&marker.verdict)
    {
        debug!(
            session_id = %marker.session_id,
            "Review verdict marker is stale for requested target"
        );
        return Ok(None);
    }

    if !review_mode_matches(marker.review_mode.as_deref(), required_mode) {
        debug!(
            session_id = %marker.session_id,
            marker_review_mode = ?marker.review_mode,
            ?required_mode,
            "Review verdict marker did not match required review mode"
        );
        return Ok(None);
    }

    let session_root = csa_session::get_session_root(project_root).with_context(|| {
        format!(
            "failed to resolve CSA session root for {}",
            project_root.display()
        )
    })?;
    let session_dir = session_root.join("sessions").join(&marker.session_id);
    if !session_dir.exists() {
        debug!(
            session_id = %marker.session_id,
            session_dir = %session_dir.display(),
            "Review verdict marker points to missing session"
        );
        return Ok(None);
    }

    let Some(meta) = read_review_meta(&session_dir)? else {
        debug!(
            session_id = %marker.session_id,
            session_dir = %session_dir.display(),
            "Review verdict marker session is missing review_meta.json"
        );
        return Ok(None);
    };
    if meta.head_sha != head_sha || meta.scope != required_scope {
        debug!(
            session_id = %marker.session_id,
            meta_head_sha = %meta.head_sha,
            expected_head_sha = head_sha,
            meta_scope = %meta.scope,
            expected_scope = required_scope,
            "Review verdict marker session did not match head SHA or scope"
        );
        return Ok(None);
    }
    if !review_mode_matches(meta.review_mode.as_deref(), required_mode) {
        debug!(
            session_id = %marker.session_id,
            meta_review_mode = ?meta.review_mode,
            ?required_mode,
            "Review verdict marker session did not match required review mode"
        );
        return Ok(None);
    }
    if !diff_fingerprint_matches(&meta, expected_diff_fingerprint) {
        debug!(
            session_id = %marker.session_id,
            meta_diff_fingerprint = ?meta.diff_fingerprint,
            ?expected_diff_fingerprint,
            "Review verdict marker session did not match diff fingerprint"
        );
        return Ok(None);
    }
    let pass_status = review_session_acceptance_status(&session_dir, &meta)?;
    if !pass_status.is_pass {
        debug!(
            session_id = %marker.session_id,
            decision = %meta.decision,
            verdict = %meta.verdict,
            "Review verdict marker session is not PASS/CLEAN"
        );
        return Ok(None);
    }

    debug!(
        session_id = %meta.session_id,
        scope = %meta.scope,
        head_sha = %meta.head_sha,
        "Review verdict marker matched PASS/CLEAN candidate"
    );
    Ok(Some(ReviewVerdictCandidate {
        session_id: meta.session_id,
        scope: meta.scope,
        head_sha: meta.head_sha,
        timestamp: meta.timestamp,
        is_pass: true,
        severity_counts: pass_status.severity_counts,
    }))
}

fn read_session_state(session_dir: &Path) -> Result<MetaSessionState> {
    let path = session_dir.join("state.toml");
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let state =
        toml::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(state)
}

struct ReviewVerdictCandidate {
    session_id: String,
    scope: String,
    head_sha: String,
    timestamp: chrono::DateTime<chrono::Utc>,
    is_pass: bool,
    severity_counts: BTreeMap<Severity, u32>,
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

/// Review mode the verdict must carry for `--check-verdict` to accept it (#1817).
///
/// Returns `Some` only when the caller explicitly selected a mode
/// (`--review-mode <mode>` or `--red-team`). When `None`, the verdict gate keeps
/// its legacy behavior and accepts a passing verdict regardless of review mode,
/// so existing callers and legacy artifacts (written before review-mode auditing)
/// continue to pass byte-for-byte.
fn required_check_verdict_mode(args: &ReviewArgs) -> Option<String> {
    if args.red_team || args.review_mode.is_some() {
        Some(args.effective_review_mode().as_str().to_string())
    } else {
        None
    }
}

/// Whether a candidate verdict's review mode satisfies the required mode.
///
/// A `None` requirement always matches (legacy / unfiltered gate). When a mode
/// is required, the candidate must carry exactly that mode; a candidate with no
/// recorded mode (legacy artifact/marker) cannot prove it ran in the required
/// mode and is therefore rejected.
fn review_mode_matches(candidate: Option<&str>, required: Option<&str>) -> bool {
    match required {
        None => true,
        Some(required_mode) => candidate == Some(required_mode),
    }
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

fn read_review_meta_after_recovery(
    project_root: &Path,
    session_id: &str,
    session_dir: &Path,
) -> Result<Option<ReviewSessionMeta>> {
    if !session_dir
        .join("output")
        .join("review-verdict.json")
        .is_file()
    {
        recover_review_sidecars_before_meta(project_root, session_id);
    }
    if let Some(meta) = read_review_meta(session_dir)? {
        return Ok(Some(meta));
    }
    recover_review_sidecars_before_meta(project_root, session_id);
    read_review_meta(session_dir)
}

fn recover_review_sidecars_before_meta(project_root: &Path, session_id: &str) {
    if let Err(error) =
        crate::session_observability::refresh_and_repair_result(project_root, session_id)
    {
        debug!(
            session_id,
            error = %error,
            "Review verdict sidecar recovery failed before metadata lookup"
        );
    }
}

fn diff_fingerprint_matches(
    meta: &ReviewSessionMeta,
    expected_diff_fingerprint: Option<&str>,
) -> bool {
    expected_diff_fingerprint
        .map(|expected| meta.diff_fingerprint.as_deref() == Some(expected))
        .unwrap_or(true)
}

struct ReviewVerdictAcceptanceStatus {
    is_pass: bool,
    severity_counts: BTreeMap<Severity, u32>,
}

fn review_session_acceptance_status(
    session_dir: &Path,
    meta: &ReviewSessionMeta,
) -> Result<ReviewVerdictAcceptanceStatus> {
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    if !verdict_path.exists() {
        debug!(
            session_id = %meta.session_id,
            meta_decision = %meta.decision,
            meta_verdict = %meta.verdict,
            verdict_path = %verdict_path.display(),
            "Rejecting review verdict because review-verdict.json is missing"
        );
        return Ok(ReviewVerdictAcceptanceStatus {
            is_pass: false,
            severity_counts: zero_severity_counts(),
        });
    }

    let raw = fs::read_to_string(&verdict_path)
        .with_context(|| format!("failed to read {}", verdict_path.display()))?;
    let artifact: ReviewVerdictArtifact = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", verdict_path.display()))?;
    let accepted = meta.accepts_clean_review_verdict(artifact.decision);
    debug!(
        session_id = %meta.session_id,
        meta_decision = %meta.decision,
        meta_verdict = %meta.verdict,
        meta_exit_code = meta.exit_code,
        meta_fix_attempted = meta.fix_attempted,
        meta_fix_converged = meta.fix_clean_converged(),
        artifact_decision = %artifact.decision,
        artifact_verdict = %artifact.verdict_legacy,
        accepted,
        verdict_path = %verdict_path.display(),
        "Read review verdict artifact"
    );
    Ok(ReviewVerdictAcceptanceStatus {
        is_pass: accepted,
        severity_counts: artifact.severity_counts,
    })
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

fn format_review_verdict_pass_message(found: &ReviewVerdictMatch, branch: &str) -> String {
    format!(
        "Review verdict check passed{}: session {} has PASS/CLEAN for {} at {} ({})",
        format_nonblocking_counts_suffix(&found.severity_counts),
        found.session_id,
        branch,
        short_sha(&found.head_sha),
        found.scope
    )
}

fn format_nonblocking_counts_suffix(counts: &BTreeMap<Severity, u32>) -> String {
    let parts = [
        (Severity::Critical, "critical"),
        (Severity::High, "high"),
        (Severity::Medium, "medium"),
        (Severity::Low, "low"),
    ]
    .into_iter()
    .filter_map(|(severity, label)| {
        let count = *counts.get(&severity).unwrap_or(&0);
        (count > 0).then(|| format!("{count} {label}"))
    })
    .collect::<Vec<_>>();

    if parts.is_empty() {
        String::new()
    } else {
        format!(" (non-blocking findings: {})", parts.join(", "))
    }
}

fn zero_severity_counts() -> BTreeMap<Severity, u32> {
    [
        (Severity::Critical, 0),
        (Severity::High, 0),
        (Severity::Medium, 0),
        (Severity::Low, 0),
    ]
    .into_iter()
    .collect()
}

#[cfg(test)]
#[path = "review_cmd_check_verdict_daemon_completion_tests.rs"]
mod daemon_completion_tests;

#[cfg(test)]
#[path = "review_cmd_check_verdict_2236_tests.rs"]
mod issue_2236_tests;

#[cfg(test)]
#[path = "review_cmd_check_verdict_2236_clean_summary_tests.rs"]
mod issue_2236_clean_summary_tests;

#[cfg(test)]
#[path = "review_cmd_check_verdict_2236_blocking_legacy_tests.rs"]
mod issue_2236_blocking_legacy_tests;

#[cfg(test)]
#[path = "review_cmd_check_verdict_compound_legacy_tests.rs"]
mod compound_legacy_tests;

#[cfg(test)]
#[path = "review_cmd_check_verdict_tests.rs"]
mod tests;
