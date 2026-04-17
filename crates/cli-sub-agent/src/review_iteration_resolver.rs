use std::path::Path;

use anyhow::{Context, Result};
use csa_session::ReviewSessionMeta;

pub(crate) fn try_max_review_iterations_for_branch(
    project_root: &Path,
    branch: &str,
    exclude_session_id: Option<&str>,
) -> Result<u32> {
    csa_session::list_sessions(project_root, None)?
        .into_iter()
        .filter(|candidate| exclude_session_id != Some(candidate.meta_session_id.as_str()))
        .filter(|candidate| candidate.resolved_identity().ref_name.as_deref() == Some(branch))
        .try_fold(0_u32, |max_review_iterations, candidate| {
            let review_iterations =
                load_review_iterations(project_root, &candidate.meta_session_id)?.unwrap_or(0);
            Ok::<u32, anyhow::Error>(max_review_iterations.max(review_iterations))
        })
}

fn load_review_iterations(project_root: &Path, session_id: &str) -> Result<Option<u32>> {
    Ok(
        load_review_meta(project_root, session_id)?
            .map(|review_meta| review_meta.review_iterations),
    )
}

pub(crate) fn load_review_meta(
    project_root: &Path,
    session_id: &str,
) -> Result<Option<ReviewSessionMeta>> {
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
