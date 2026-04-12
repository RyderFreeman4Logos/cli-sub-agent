use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use csa_session::ReviewSessionMeta;
use csa_session::review_artifact::ReviewArtifact;
use tracing::{info, warn};

use crate::bug_class::{
    SkillExtractor, classify_recurring_bug_classes, load_review_artifacts_for_project,
};
use crate::review_consensus::build_consolidated_artifact;

const REVIEW_CONSOLIDATED_ARTIFACT_FILE: &str = "review-consolidated.json";
const REVIEW_FINDINGS_ARTIFACT_FILE: &str = "review-findings.json";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ReviewArtifactGroupKey {
    branch: String,
    review_iterations: u32,
    scope: String,
    head_sha: String,
    diff_fingerprint: Option<String>,
}

struct GroupedReviewArtifact {
    artifact: ReviewArtifact,
    has_consolidated_artifact: bool,
}

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

    let review_artifacts = load_bug_class_review_artifacts(project_root)?;
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

pub(super) fn load_bug_class_review_artifacts(project_root: &Path) -> Result<Vec<ReviewArtifact>> {
    let review_artifacts = load_review_artifacts_for_project(project_root)?;
    collapse_bug_class_review_artifacts(project_root, review_artifacts)
}

fn collapse_bug_class_review_artifacts(
    project_root: &Path,
    review_artifacts: Vec<ReviewArtifact>,
) -> Result<Vec<ReviewArtifact>> {
    let mut collapsed = Vec::new();
    let mut grouped = BTreeMap::<ReviewArtifactGroupKey, Vec<GroupedReviewArtifact>>::new();

    for artifact in review_artifacts {
        // Parent review-consolidated.json duplicates child reviewer findings but
        // does not carry review_meta.json. Skip only that parent artifact shape;
        // child reviewer sessions can legitimately have consolidated output too.
        if should_skip_bug_class_artifact(project_root, &artifact.session_id)? {
            continue;
        }

        let Some(group_key) =
            resolve_review_artifact_group_key(project_root, &artifact.session_id)?
        else {
            collapsed.push(artifact);
            continue;
        };

        grouped
            .entry(group_key)
            .or_default()
            .push(GroupedReviewArtifact {
                has_consolidated_artifact: session_has_consolidated_artifact(
                    project_root,
                    &artifact.session_id,
                )?,
                artifact,
            });
    }

    for (_, artifacts) in grouped {
        if let Some(consolidated) = artifacts
            .iter()
            .find(|artifact| artifact.has_consolidated_artifact)
            .map(|artifact| artifact.artifact.clone())
        {
            collapsed.push(consolidated);
            continue;
        }

        let mut artifacts = artifacts
            .into_iter()
            .map(|artifact| artifact.artifact)
            .collect::<Vec<_>>();
        if artifacts.len() == 1 {
            collapsed.extend(artifacts);
            continue;
        }

        let session_id = artifacts
            .first()
            .map(|artifact| artifact.session_id.clone())
            .unwrap_or_else(|| "unknown".to_string());
        collapsed.push(build_consolidated_artifact(
            std::mem::take(&mut artifacts),
            &session_id,
        ));
    }

    Ok(collapsed)
}

fn should_skip_bug_class_artifact(project_root: &Path, session_id: &str) -> Result<bool> {
    Ok(session_has_consolidated_artifact(project_root, session_id)?
        && load_review_meta(project_root, session_id)?.is_none())
}

fn resolve_review_artifact_group_key(
    project_root: &Path,
    session_id: &str,
) -> Result<Option<ReviewArtifactGroupKey>> {
    let Some(review_meta) = load_review_meta(project_root, session_id)? else {
        return Ok(None);
    };
    let session = csa_session::load_session(project_root, session_id)
        .with_context(|| format!("failed to load review session {session_id}"))?;
    let Some(branch) = session.resolved_identity().ref_name else {
        return Ok(None);
    };

    Ok(Some(ReviewArtifactGroupKey {
        branch,
        review_iterations: review_meta.review_iterations,
        scope: review_meta.scope,
        head_sha: review_meta.head_sha,
        diff_fingerprint: review_meta.diff_fingerprint,
    }))
}

fn session_has_consolidated_artifact(project_root: &Path, session_id: &str) -> Result<bool> {
    let session_dir = csa_session::get_session_dir(project_root, session_id)
        .with_context(|| format!("failed to resolve review session dir for {session_id}"))?;
    Ok(session_dir
        .join(REVIEW_CONSOLIDATED_ARTIFACT_FILE)
        .is_file())
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

    let max_review_iterations = csa_session::list_sessions(project_root, None)?
        .into_iter()
        .filter(|candidate| candidate.meta_session_id != session_id)
        .filter(|candidate| {
            candidate.resolved_identity().ref_name.as_deref() == Some(branch.as_str())
        })
        .try_fold(0_u32, |max_review_iterations, candidate| {
            let review_iterations =
                load_review_iterations(project_root, &candidate.meta_session_id)?.unwrap_or(0);
            Ok::<u32, anyhow::Error>(max_review_iterations.max(review_iterations))
        })?;

    Ok(std::cmp::max(1, max_review_iterations.saturating_add(1)))
}

fn load_review_iterations(project_root: &Path, session_id: &str) -> Result<Option<u32>> {
    Ok(
        load_review_meta(project_root, session_id)?
            .map(|review_meta| review_meta.review_iterations),
    )
}

fn load_review_meta(project_root: &Path, session_id: &str) -> Result<Option<ReviewSessionMeta>> {
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
    Ok(Some(review_meta))
}
