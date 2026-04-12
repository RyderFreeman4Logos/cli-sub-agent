use std::path::Path;

use anyhow::{Context, Result};
use csa_session::ReviewSessionMeta;
use csa_session::review_artifact::ReviewArtifact;
use tracing::{info, warn};

use crate::bug_class::{
    SkillExtractor, classify_recurring_bug_classes, load_review_artifacts_for_project,
};

const REVIEW_CONSOLIDATED_ARTIFACT_FILE: &str = "review-consolidated.json";
const REVIEW_FINDINGS_ARTIFACT_FILE: &str = "review-findings.json";

pub(super) fn maybe_extract_recurring_bug_class_skills(
    project_root: &Path,
    session_ids: &[String],
) {
    if let Err(err) = try_extract_recurring_bug_class_skills(project_root, session_ids) {
        warn!(
            error = %err,
            session_ids = ?session_ids,
            "Failed to extract recurring bug-class skills after review completion"
        );
    }
}

pub(super) fn try_extract_recurring_bug_class_skills(
    project_root: &Path,
    session_ids: &[String],
) -> Result<()> {
    let mut should_extract = false;
    for session_id in session_ids {
        if session_has_high_or_critical_review_findings(project_root, session_id)? {
            should_extract = true;
            break;
        }
    }
    if !should_extract {
        return Ok(());
    }

    let review_artifacts = load_review_artifacts_for_project(project_root)?;
    let candidates = classify_recurring_bug_classes(&review_artifacts);
    if candidates.is_empty() {
        return Ok(());
    }

    let written = SkillExtractor::from_global_config()?.extract(&candidates)?;
    if !written.is_empty() {
        let generated_skills = written
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();
        info!(
            recurring_bug_classes = candidates.len(),
            generated_skills = ?generated_skills,
            "Generated recurring review code-quality skills"
        );
    }

    Ok(())
}

fn session_has_high_or_critical_review_findings(
    project_root: &Path,
    session_id: &str,
) -> Result<bool> {
    let session_dir = csa_session::get_session_dir(project_root, session_id)
        .with_context(|| format!("failed to resolve review session dir for {session_id}"))?;
    let Some(artifact) = load_session_review_artifact(&session_dir)? else {
        return Ok(false);
    };

    Ok(artifact.severity_summary.critical > 0 || artifact.severity_summary.high > 0)
}

fn load_session_review_artifact(session_dir: &Path) -> Result<Option<ReviewArtifact>> {
    for artifact_file in [
        REVIEW_CONSOLIDATED_ARTIFACT_FILE,
        REVIEW_FINDINGS_ARTIFACT_FILE,
    ] {
        let artifact_path = session_dir.join(artifact_file);
        if !artifact_path.is_file() {
            continue;
        }

        let artifact_content = std::fs::read_to_string(&artifact_path).with_context(|| {
            format!("failed to read review artifact {}", artifact_path.display())
        })?;
        let artifact = serde_json::from_str(&artifact_content).with_context(|| {
            format!(
                "failed to parse review artifact {}",
                artifact_path.display()
            )
        })?;
        return Ok(Some(artifact));
    }

    Ok(None)
}

pub(super) fn resolve_review_iterations(project_root: &Path, session_id: &str) -> u32 {
    match try_resolve_review_iterations(project_root, session_id) {
        Ok(review_iterations) => review_iterations,
        Err(err) => {
            warn!(
                session_id,
                error = %err,
                "Failed to resolve review_iterations; defaulting to 1"
            );
            1
        }
    }
}

pub(super) fn try_resolve_review_iterations(project_root: &Path, session_id: &str) -> Result<u32> {
    let session = csa_session::load_session(project_root, session_id)
        .with_context(|| format!("failed to load review session {session_id}"))?;
    let Some(branch) = session.resolved_identity().ref_name else {
        return Ok(1);
    };

    let mut branch_sessions = csa_session::list_sessions(project_root, None)?
        .into_iter()
        .filter(|candidate| candidate.meta_session_id != session_id)
        .filter(|candidate| {
            candidate.resolved_identity().ref_name.as_deref() == Some(branch.as_str())
        })
        .collect::<Vec<_>>();
    branch_sessions.sort_by(|left, right| right.last_accessed.cmp(&left.last_accessed));

    for candidate in branch_sessions {
        if let Some(review_iterations) =
            load_review_iterations(project_root, &candidate.meta_session_id)?
        {
            return Ok(review_iterations.saturating_add(1));
        }
    }

    Ok(1)
}

fn load_review_iterations(project_root: &Path, session_id: &str) -> Result<Option<u32>> {
    let session_dir = csa_session::get_session_dir(project_root, session_id)
        .with_context(|| format!("failed to resolve review session dir for {session_id}"))?;
    let review_meta_path = session_dir.join("review_meta.json");
    if !review_meta_path.is_file() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&review_meta_path)
        .with_context(|| format!("failed to read {}", review_meta_path.display()))?;
    let review_meta: ReviewSessionMeta = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse {}", review_meta_path.display()))?;
    Ok(Some(review_meta.review_iterations))
}
