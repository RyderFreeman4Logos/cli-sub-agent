use std::path::Path;

use tracing::debug;

pub(super) fn refresh_structured_output_before_verdict(session_dir: &Path, session_id: &str) {
    let output_log = session_dir.join("output.log");
    if !output_log.is_file() {
        return;
    }
    if let Err(error) = csa_session::persist_structured_output_from_file(session_dir, &output_log) {
        debug!(
            session_id,
            path = %output_log.display(),
            error = %error,
            "Failed to refresh structured output before review verdict derivation"
        );
    }
}
