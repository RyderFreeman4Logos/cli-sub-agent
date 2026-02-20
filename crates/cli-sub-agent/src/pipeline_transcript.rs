use std::fs;
use std::path::Path;

use csa_config::ProjectConfig;
use csa_executor::TransportResult;
use csa_session::{EventWriter, SessionArtifact};
use tracing::warn;

pub(crate) fn persist_if_enabled(
    config: Option<&ProjectConfig>,
    session_dir: &Path,
    transport_result: &TransportResult,
) -> Vec<SessionArtifact> {
    if !config.is_some_and(|cfg| cfg.session.transcript_enabled) {
        return Vec::new();
    }

    let transcript_rel_path = "output/acp-events.jsonl";
    let transcript_path = session_dir.join(transcript_rel_path);
    let mut event_writer = EventWriter::new(&transcript_path);
    event_writer.append_all(transport_result.events.iter());
    event_writer.flush();

    let stats = event_writer.stats();
    if stats.write_failures > 0 {
        warn!(
            path = %transcript_path.display(),
            write_failures = stats.write_failures,
            "ACP transcript writer reported failures"
        );
    }

    match fs::metadata(&transcript_path) {
        Ok(metadata) => vec![SessionArtifact::with_stats(
            transcript_rel_path.to_string(),
            stats.lines_written,
            metadata.len(),
        )],
        Err(err) => {
            warn!(
                path = %transcript_path.display(),
                error = %err,
                "ACP transcript metadata is unavailable"
            );
            Vec::new()
        }
    }
}
