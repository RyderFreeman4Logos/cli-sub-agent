use std::path::Path;

use csa_session::ReviewVerdictArtifact;
use csa_session::state::ReviewSessionMeta;
use tracing::warn;

pub(super) fn attach_no_provider_launch_diagnostic(
    session_dir: &Path,
    meta: &ReviewSessionMeta,
    artifact: &mut ReviewVerdictArtifact,
) {
    let Ok(Some(mut diagnostic)) = csa_session::read_no_provider_launch_diagnostic(session_dir)
    else {
        return;
    };

    crate::no_provider_launch::enrich_review_diagnostic(
        &mut diagnostic,
        &meta.head_sha,
        &meta.scope,
    );
    if let Err(error) = csa_session::write_no_provider_launch_diagnostic(session_dir, &diagnostic) {
        warn!(
            session_id = %meta.session_id,
            error = %error,
            "Failed to rewrite enriched no-provider diagnostic"
        );
    }
    artifact.no_provider_launch = Some(diagnostic);
}
