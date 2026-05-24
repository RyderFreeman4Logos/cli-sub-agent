use std::fs;
use std::path::Path;

use csa_session::ReviewVerdictArtifact;
use tracing::warn;

pub(in crate::review_cmd) fn persisted_review_verdict_exit_code(
    project_root: &Path,
    session_id: &str,
) -> i32 {
    let session_dir = match csa_session::get_session_dir(project_root, session_id) {
        Ok(session_dir) => session_dir,
        Err(error) => {
            warn!(
                session_id,
                error = %error,
                "Cannot resolve session dir for persisted review verdict; treating as infrastructure failure"
            );
            return crate::verdict_exit_code::INFRASTRUCTURE_FAILURE_EXIT_CODE;
        }
    };
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let raw = match fs::read_to_string(&verdict_path) {
        Ok(raw) => raw,
        Err(error) => {
            warn!(
                session_id,
                path = %verdict_path.display(),
                error = %error,
                "Missing or unreadable review verdict artifact; treating as infrastructure failure"
            );
            return crate::verdict_exit_code::INFRASTRUCTURE_FAILURE_EXIT_CODE;
        }
    };
    let artifact = match serde_json::from_str::<ReviewVerdictArtifact>(&raw) {
        Ok(artifact) => artifact,
        Err(error) => {
            warn!(
                session_id,
                path = %verdict_path.display(),
                error = %error,
                "Invalid review verdict artifact; treating as infrastructure failure"
            );
            return crate::verdict_exit_code::INFRASTRUCTURE_FAILURE_EXIT_CODE;
        }
    };

    crate::verdict_exit_code::exit_code_from_review_decision(artifact.decision)
}

pub(in crate::review_cmd) fn persist_review_result_exit_code(
    project_root: &Path,
    session_id: &str,
    exit_code: i32,
) {
    let mut result = match csa_session::load_result(project_root, session_id) {
        Ok(Some(result)) => result,
        Ok(None) => return,
        Err(error) => {
            warn!(
                session_id,
                error = %error,
                "Failed to load review result.toml for verdict exit-code alignment"
            );
            return;
        }
    };
    if result.exit_code == exit_code
        && result.status == csa_session::SessionResult::status_from_exit_code(exit_code)
    {
        return;
    }

    result.exit_code = exit_code;
    result.status = csa_session::SessionResult::status_from_exit_code(exit_code);
    if let Err(error) = csa_session::save_result(project_root, session_id, &result) {
        warn!(
            session_id,
            error = %error,
            "Failed to persist review result.toml verdict exit-code alignment"
        );
    }
}
