use std::path::Path;

use csa_config::{ProjectConfig, ProjectProfile};
use csa_core::types::FallbackAttempt;
use tracing::{debug, warn};

#[derive(Debug, Clone)]
pub(crate) struct ReviewRoutingMetadata {
    pub(crate) project_profile: ProjectProfile,
    pub(crate) detection_method: &'static str,
}

pub(crate) fn detect_review_routing_metadata(
    project_root: &Path,
    _project_config: Option<&ProjectConfig>,
) -> ReviewRoutingMetadata {
    // Project-level profile override is not part of ProjectConfig schema yet.
    let project_profile = csa_config::detect_project_profile(project_root);
    ReviewRoutingMetadata {
        project_profile,
        detection_method: "auto",
    }
}

pub(crate) fn persist_review_routing_artifact(
    project_root: &Path,
    meta_session_id: &str,
    review_routing: &ReviewRoutingMetadata,
) {
    persist_review_routing_artifact_with_fallback_chain(
        project_root,
        meta_session_id,
        review_routing,
        &[],
    );
}

pub(crate) fn persist_review_routing_artifact_with_fallback_chain(
    project_root: &Path,
    meta_session_id: &str,
    review_routing: &ReviewRoutingMetadata,
    fallback_chain: &[FallbackAttempt],
) {
    let session_dir = match csa_session::get_session_dir(project_root, meta_session_id) {
        Ok(path) => path,
        Err(err) => {
            debug!(
                session_id = %meta_session_id,
                error = %err,
                "Skipping review-routing artifact write: failed to resolve session directory"
            );
            return;
        }
    };

    let output_dir = session_dir.join("output");
    if !output_dir.is_dir() {
        debug!(
            session_id = %meta_session_id,
            output_dir = %output_dir.display(),
            "Skipping review-routing artifact write: output directory missing"
        );
        return;
    }

    let artifact = render_review_routing_artifact(review_routing, fallback_chain);
    let artifact_path = output_dir.join("review-routing.json");
    if let Err(err) = std::fs::write(&artifact_path, artifact) {
        warn!(
            session_id = %meta_session_id,
            path = %artifact_path.display(),
            error = %err,
            "Failed to write review-routing artifact (best-effort)"
        );
    }
}

pub(crate) fn render_review_routing_artifact(
    review_routing: &ReviewRoutingMetadata,
    fallback_chain: &[FallbackAttempt],
) -> String {
    let mut artifact = serde_json::json!({
        "project_profile": review_routing.project_profile.to_string(),
        "detection_method": review_routing.detection_method,
        "schema_version": "1.0",
    });
    if !fallback_chain.is_empty() {
        artifact["fallback_chain"] =
            serde_json::to_value(fallback_chain).unwrap_or_else(|_| serde_json::json!([]));
    }
    format!("{artifact}\n")
}
