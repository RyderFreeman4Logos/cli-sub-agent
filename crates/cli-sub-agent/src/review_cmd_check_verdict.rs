use std::fs;
use std::path::Path;
use std::str::FromStr;

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

pub(crate) fn handle_check_verdict(project_root: &Path) -> Result<i32> {
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

    let diff_fingerprint =
        super::execute::compute_diff_fingerprint(project_root, REQUIRED_FULL_DIFF_SCOPE);

    match check_review_verdict_for_target(
        project_root,
        &branch,
        &head_sha,
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
                REQUIRED_FULL_DIFF_SCOPE,
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
        if session.genealogy.parent_session_id.is_some() {
            debug!(
                session_id = %session.meta_session_id,
                parent_session_id = ?session.genealogy.parent_session_id,
                "Skipping review verdict session: child reviewer session"
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
        if meta.head_sha != head_sha || meta.scope != REQUIRED_FULL_DIFF_SCOPE {
            debug!(
                session_id = %session.meta_session_id,
                meta_head_sha = %meta.head_sha,
                expected_head_sha = head_sha,
                meta_scope = %meta.scope,
                expected_scope = REQUIRED_FULL_DIFF_SCOPE,
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
        if !review_meta_or_artifact_is_pass(&session_dir, &meta)? {
            debug!(
                session_id = %session.meta_session_id,
                decision = %meta.decision,
                verdict = %meta.verdict,
                "Skipping review verdict session: no PASS/CLEAN verdict"
            );
            continue;
        }
        debug!(
            session_id = %session.meta_session_id,
            scope = %meta.scope,
            head_sha = %meta.head_sha,
            "Found matching PASS/CLEAN review verdict"
        );
        return Ok(Some(ReviewVerdictMatch {
            session_id: meta.session_id,
            scope: meta.scope,
            head_sha: meta.head_sha,
        }));
    }

    Ok(None)
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
mod tests {
    use super::*;
    use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
    use chrono::Utc;
    use csa_core::vcs::{VcsIdentity, VcsKind};
    use tempfile::TempDir;

    fn write_review_session(
        project_root: &Path,
        branch: &str,
        head_sha: &str,
        scope: &str,
        decision: ReviewDecision,
        legacy_verdict: &str,
    ) -> String {
        write_review_session_with_parent(
            project_root,
            branch,
            head_sha,
            scope,
            decision,
            legacy_verdict,
            None,
        )
    }

    fn write_review_session_with_parent(
        project_root: &Path,
        branch: &str,
        head_sha: &str,
        scope: &str,
        decision: ReviewDecision,
        legacy_verdict: &str,
        parent_id: Option<&str>,
    ) -> String {
        let mut session =
            csa_session::create_session_fresh(project_root, Some("review: test"), parent_id, None)
                .expect("create session");
        session.branch = Some(branch.to_string());
        session.git_head_at_creation = Some(head_sha.to_string());
        session.vcs_identity = Some(VcsIdentity {
            vcs_kind: VcsKind::Git,
            commit_id: Some(head_sha.to_string()),
            change_id: None,
            short_id: Some(short_sha(head_sha).to_string()),
            ref_name: Some(branch.to_string()),
            op_id: None,
        });
        csa_session::save_session(&session).expect("save session state");

        let session_dir =
            csa_session::get_session_dir(project_root, &session.meta_session_id).unwrap();
        let meta = ReviewSessionMeta {
            session_id: session.meta_session_id.clone(),
            head_sha: head_sha.to_string(),
            decision: decision.as_str().to_string(),
            verdict: legacy_verdict.to_string(),
            status_reason: None,
            routed_to: None,
            primary_failure: None,
            failure_reason: None,
            tool: "codex".to_string(),
            scope: scope.to_string(),
            exit_code: if decision == ReviewDecision::Pass {
                0
            } else {
                1
            },
            fix_attempted: false,
            fix_rounds: 0,
            review_iterations: 1,
            timestamp: Utc::now(),
            diff_fingerprint: None,
        };
        csa_session::state::write_review_meta(&session_dir, &meta).expect("write review meta");
        csa_session::write_review_verdict(
            &session_dir,
            &ReviewVerdictArtifact::from_parts(
                session.meta_session_id.clone(),
                decision,
                legacy_verdict,
                &[],
                Vec::new(),
            ),
        )
        .expect("write review verdict");

        session.meta_session_id
    }

    fn set_review_diff_fingerprint(
        project_root: &Path,
        session_id: &str,
        diff_fingerprint: Option<&str>,
    ) {
        let session_dir = csa_session::get_session_dir(project_root, session_id).unwrap();
        let mut meta = read_review_meta(&session_dir)
            .unwrap()
            .expect("review meta should exist");
        meta.diff_fingerprint = diff_fingerprint.map(str::to_string);
        csa_session::state::write_review_meta(&session_dir, &meta).expect("write review meta");
    }

    #[test]
    fn check_verdict_finds_pass_for_current_branch_head_and_full_diff() {
        let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
        let temp = TempDir::new().unwrap();
        let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();

        let session_id = write_review_session(
            &project,
            "feature",
            "abcdef1234567890",
            REQUIRED_FULL_DIFF_SCOPE,
            ReviewDecision::Pass,
            "CLEAN",
        );

        let found = check_review_verdict_for_target(&project, "feature", "abcdef1234567890", None)
            .unwrap()
            .expect("expected matching verdict");
        assert_eq!(found.session_id, session_id);
    }

    #[test]
    fn check_verdict_rejects_stale_diff_fingerprint() {
        let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
        let temp = TempDir::new().unwrap();
        let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();

        let session_id = write_review_session(
            &project,
            "feature",
            "abcdef1234567890",
            REQUIRED_FULL_DIFF_SCOPE,
            ReviewDecision::Pass,
            "CLEAN",
        );
        set_review_diff_fingerprint(&project, &session_id, Some("sha256:old"));

        let found = check_review_verdict_for_target(
            &project,
            "feature",
            "abcdef1234567890",
            Some("sha256:new"),
        )
        .unwrap();
        assert!(found.is_none(), "stale diff review must not satisfy gate");

        set_review_diff_fingerprint(&project, &session_id, Some("sha256:new"));
        let found = check_review_verdict_for_target(
            &project,
            "feature",
            "abcdef1234567890",
            Some("sha256:new"),
        )
        .unwrap()
        .expect("matching diff fingerprint should satisfy gate");
        assert_eq!(found.session_id, session_id);
    }

    #[test]
    fn check_verdict_rejects_child_pass_when_parent_consensus_fails() {
        let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
        let temp = TempDir::new().unwrap();
        let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();

        let parent_session_id = write_review_session(
            &project,
            "feature",
            "abcdef1234567890",
            REQUIRED_FULL_DIFF_SCOPE,
            ReviewDecision::Fail,
            "HAS_ISSUES",
        );
        write_review_session_with_parent(
            &project,
            "feature",
            "abcdef1234567890",
            REQUIRED_FULL_DIFF_SCOPE,
            ReviewDecision::Pass,
            "CLEAN",
            Some(&parent_session_id),
        );

        let found =
            check_review_verdict_for_target(&project, "feature", "abcdef1234567890", None).unwrap();
        assert!(found.is_none(), "child reviewer pass must not satisfy gate");
    }

    #[test]
    fn check_verdict_rejects_non_pass_artifact_even_when_meta_is_pass() {
        let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
        for (decision, legacy_verdict) in [
            (ReviewDecision::Fail, "HAS_ISSUES"),
            (ReviewDecision::Uncertain, "UNCERTAIN"),
            (ReviewDecision::Skip, "SKIP"),
            (ReviewDecision::Unavailable, "UNAVAILABLE"),
        ] {
            let temp = TempDir::new().unwrap();
            let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
            let project = temp.path().join("project");
            std::fs::create_dir_all(&project).unwrap();

            let session_id = write_review_session(
                &project,
                "feature",
                "abcdef1234567890",
                REQUIRED_FULL_DIFF_SCOPE,
                ReviewDecision::Pass,
                "CLEAN",
            );
            let session_dir = csa_session::get_session_dir(&project, &session_id).unwrap();
            csa_session::write_review_verdict(
                &session_dir,
                &ReviewVerdictArtifact::from_parts(
                    session_id,
                    decision,
                    legacy_verdict,
                    &[],
                    Vec::new(),
                ),
            )
            .expect("write non-pass review verdict");

            let found =
                check_review_verdict_for_target(&project, "feature", "abcdef1234567890", None)
                    .unwrap();
            assert!(
                found.is_none(),
                "expected no match for non-pass artifact decision {decision}"
            );
        }
    }

    #[test]
    fn check_verdict_does_not_recover_or_rewrite_corrupt_session_state() {
        let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
        let temp = TempDir::new().unwrap();
        let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();

        let corrupt_session =
            csa_session::create_session_fresh(&project, Some("corrupt session"), None, None)
                .expect("create corrupt session");
        let corrupt_dir =
            csa_session::get_session_dir(&project, &corrupt_session.meta_session_id).unwrap();
        let corrupt_state_path = corrupt_dir.join("state.toml");
        let corrupt_state = "this is not valid toml";
        std::fs::write(&corrupt_state_path, corrupt_state).expect("corrupt state");

        let session_id = write_review_session(
            &project,
            "feature",
            "abcdef1234567890",
            REQUIRED_FULL_DIFF_SCOPE,
            ReviewDecision::Pass,
            "CLEAN",
        );

        let found = check_review_verdict_for_target(&project, "feature", "abcdef1234567890", None)
            .unwrap()
            .expect("expected matching verdict despite unrelated corrupt session");
        assert_eq!(found.session_id, session_id);
        assert_eq!(
            std::fs::read_to_string(&corrupt_state_path).expect("read corrupt state"),
            corrupt_state
        );
        assert!(!corrupt_dir.join("state.toml.corrupt").exists());
    }

    #[test]
    fn check_verdict_rejects_commit_review_even_when_clean() {
        let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
        let temp = TempDir::new().unwrap();
        let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();

        write_review_session(
            &project,
            "feature",
            "abcdef1234567890",
            "commit:abcdef1234567890",
            ReviewDecision::Pass,
            "CLEAN",
        );

        let found =
            check_review_verdict_for_target(&project, "feature", "abcdef1234567890", None).unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn check_verdict_rejects_stale_head() {
        let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
        let temp = TempDir::new().unwrap();
        let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", temp.path().join("state"));
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();

        write_review_session(
            &project,
            "feature",
            "old1234567890",
            REQUIRED_FULL_DIFF_SCOPE,
            ReviewDecision::Pass,
            "CLEAN",
        );

        let found =
            check_review_verdict_for_target(&project, "feature", "new1234567890", None).unwrap();
        assert!(found.is_none());
    }
}
