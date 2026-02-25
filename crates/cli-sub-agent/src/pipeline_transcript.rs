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
    let redaction_enabled = config
        .map(|cfg| cfg.session.transcript_redaction)
        .unwrap_or(true);
    let mut event_writer = EventWriter::with_redaction(&transcript_path, redaction_enabled);
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

#[cfg(test)]
mod tests {
    use super::*;
    use csa_acp::SessionEvent;
    use csa_config::config::CURRENT_SCHEMA_VERSION;
    use csa_config::{ProjectMeta, ResourcesConfig, SessionConfig};
    use csa_process::ExecutionResult;
    use std::collections::HashMap;

    fn config_with_transcript_enabled(enabled: bool) -> ProjectConfig {
        ProjectConfig {
            schema_version: CURRENT_SCHEMA_VERSION,
            project: ProjectMeta::default(),
            resources: ResourcesConfig::default(),
            acp: Default::default(),
            session: SessionConfig {
                transcript_enabled: enabled,
                transcript_redaction: true,
                structured_output: true,
                ..Default::default()
            },
            tools: HashMap::new(),
            review: None,
            debate: None,
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
            preferences: None,
            memory: Default::default(),
        }
    }

    fn transport_result_with_events(events: Vec<SessionEvent>) -> TransportResult {
        TransportResult {
            execution: ExecutionResult {
                output: String::new(),
                stderr_output: String::new(),
                summary: String::new(),
                exit_code: 0,
            },
            provider_session_id: None,
            events,
        }
    }

    #[test]
    fn test_persist_if_enabled_disabled_does_not_write_transcript_file() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = config_with_transcript_enabled(false);
        let transport_result =
            transport_result_with_events(vec![SessionEvent::AgentMessage("hello".to_string())]);

        let artifacts = persist_if_enabled(Some(&cfg), tmp.path(), &transport_result);

        assert!(artifacts.is_empty());
        assert!(!tmp.path().join("output").join("acp-events.jsonl").exists());
        assert_eq!(transport_result.events.len(), 1);
    }

    #[test]
    fn test_persist_if_enabled_handles_empty_events_gracefully() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = config_with_transcript_enabled(true);
        let transport_result = transport_result_with_events(Vec::new());

        let artifacts = persist_if_enabled(Some(&cfg), tmp.path(), &transport_result);
        let transcript_path = tmp.path().join("output").join("acp-events.jsonl");

        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].path, "output/acp-events.jsonl");
        assert_eq!(artifacts[0].line_count, Some(0));
        assert!(transcript_path.exists());
    }

    #[test]
    fn test_persist_if_enabled_gracefully_handles_writer_creation_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let blocked_path = tmp.path().join("blocked");
        fs::write(&blocked_path, "not-a-directory").unwrap();

        let cfg = config_with_transcript_enabled(true);
        let transport_result =
            transport_result_with_events(vec![SessionEvent::AgentMessage("hello".to_string())]);

        let artifacts = persist_if_enabled(Some(&cfg), &blocked_path, &transport_result);

        assert!(artifacts.is_empty());
    }
}
