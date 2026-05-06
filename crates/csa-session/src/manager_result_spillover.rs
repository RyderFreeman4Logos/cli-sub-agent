use super::*;

const LARGE_OUTPUT_ARTIFACT_PATH: &str = "artifacts/large-output-report.md";
const REPORT_SUMMARY_PREVIEW_CHARS: usize = 240;
const REPORT_BODY_PREVIEW_CHARS: usize = 480;
const REPORT_DECISION_PREVIEW_CHARS: usize = 160;

pub(super) fn resolve_report_spill_threshold_bytes(project_path: &Path) -> u64 {
    match csa_config::ProjectConfig::load(project_path) {
        Ok(Some(config)) => config.session.result_report_spill_threshold_bytes,
        Ok(None) => csa_config::DEFAULT_RESULT_REPORT_SPILL_THRESHOLD_BYTES,
        Err(error) => {
            tracing::warn!(
                path = %project_path.display(),
                error = %error,
                "Failed to load session spillover config; using default threshold"
            );
            csa_config::DEFAULT_RESULT_REPORT_SPILL_THRESHOLD_BYTES
        }
    }
}

pub(super) fn load_existing_manager_sidecar_for_publish(
    session_dir: &Path,
    result: &mut SessionResult,
    spill_threshold_bytes: u64,
) -> Result<Option<toml::Value>> {
    let sidecar_path = session_dir.join(CONTRACT_RESULT_ARTIFACT_PATH);
    match load_optional_result_sidecar(session_dir, CONTRACT_RESULT_ARTIFACT_PATH) {
        Ok(Some(sidecar)) => Ok(Some(prepare_manager_sidecar_for_publish(
            session_dir,
            result,
            sidecar,
            spill_threshold_bytes,
        )?)),
        Ok(None) => Ok(None),
        Err(error) => {
            tracing::warn!(
                path = %sidecar_path.display(),
                error = %error,
                "Failed to load existing manager sidecar for spillover processing; preserving original file"
            );
            Ok(None)
        }
    }
}

pub(super) fn prepare_manager_sidecar_for_publish(
    session_dir: &Path,
    result: &mut SessionResult,
    mut sidecar: toml::Value,
    spill_threshold_bytes: u64,
) -> Result<toml::Value> {
    if spill_threshold_bytes == 0 {
        return Ok(sidecar);
    }

    let Some(sidecar_table) = sidecar.as_table_mut() else {
        return Ok(sidecar);
    };
    let Some(report_table) = sidecar_table
        .get_mut("report")
        .and_then(toml::Value::as_table_mut)
    else {
        return Ok(sidecar);
    };

    let summary = report_table
        .get("summary")
        .and_then(toml::Value::as_str)
        .map(ToOwned::to_owned);
    let what_was_done = report_table
        .get("what_was_done")
        .and_then(toml::Value::as_str)
        .map(ToOwned::to_owned);
    let key_decisions = report_table
        .get("key_decisions")
        .and_then(toml::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(toml::Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let combined_bytes = summary.as_deref().map_or(0, str::len)
        + what_was_done.as_deref().map_or(0, str::len)
        + key_decisions.iter().map(String::len).sum::<usize>();
    if combined_bytes as u64 <= spill_threshold_bytes {
        return Ok(sidecar);
    }

    let artifact_contract_path = format!("$CSA_SESSION_DIR/{LARGE_OUTPUT_ARTIFACT_PATH}");
    let artifact_content = render_large_output_artifact(
        spill_threshold_bytes,
        combined_bytes as u64,
        summary.as_deref(),
        what_was_done.as_deref(),
        &key_decisions,
    );
    let artifact_disk_path = session_dir.join(LARGE_OUTPUT_ARTIFACT_PATH);
    write_file_atomically(&artifact_disk_path, &artifact_content).with_context(|| {
        format!(
            "Failed to write large output spillover artifact: {}",
            artifact_disk_path.display()
        )
    })?;
    let artifact_size_bytes = artifact_content.len() as u64;
    upsert_session_artifact(
        result,
        LARGE_OUTPUT_ARTIFACT_PATH,
        Some(artifact_size_bytes),
    );

    let reverse_prompt = format!(
        "Run `rg <pattern> {artifact_contract_path}` to search findings, or `head -100 {artifact_contract_path}` for summary"
    );
    let summary_source = summary
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            what_was_done
                .as_deref()
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| {
            key_decisions
                .iter()
                .find(|value| !value.trim().is_empty())
                .map(String::as_str)
        })
        .unwrap_or("Manager-facing report spilled to artifact");
    let summary_preview = format!(
        "{} [full report: {}]",
        truncate_with_ellipsis(summary_source, REPORT_SUMMARY_PREVIEW_CHARS),
        artifact_contract_path
    );
    report_table.insert("summary".to_string(), toml::Value::String(summary_preview));
    if let Some(full_what_was_done) = what_was_done.as_deref() {
        report_table.insert(
            "what_was_done".to_string(),
            toml::Value::String(format!(
                "{} [truncated; full report: {}]",
                truncate_with_ellipsis(full_what_was_done, REPORT_BODY_PREVIEW_CHARS),
                artifact_contract_path
            )),
        );
    }
    if !key_decisions.is_empty() {
        let mut decision_preview = key_decisions
            .iter()
            .take(3)
            .map(|value| truncate_with_ellipsis(value, REPORT_DECISION_PREVIEW_CHARS))
            .collect::<Vec<_>>();
        if key_decisions.len() > 3
            || key_decisions
                .iter()
                .take(3)
                .zip(decision_preview.iter())
                .any(|(original, truncated)| original != truncated)
        {
            decision_preview.push(format!(
                "Additional decisions truncated; full list: {artifact_contract_path}"
            ));
        }
        report_table.insert(
            "key_decisions".to_string(),
            toml::Value::Array(
                decision_preview
                    .into_iter()
                    .map(toml::Value::String)
                    .collect(),
            ),
        );
    }

    let artifacts_table = sidecar_table
        .entry("artifacts".to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    let Some(artifacts_table) = artifacts_table.as_table_mut() else {
        return Ok(sidecar);
    };
    artifacts_table.insert(
        "large_output_path".to_string(),
        toml::Value::String(artifact_contract_path),
    );
    artifacts_table.insert(
        "large_output_size_bytes".to_string(),
        toml::Value::Integer(artifact_size_bytes as i64),
    );
    artifacts_table.insert(
        "reverse_prompt".to_string(),
        toml::Value::String(reverse_prompt),
    );

    Ok(sidecar)
}

fn render_large_output_artifact(
    threshold_bytes: u64,
    combined_bytes: u64,
    summary: Option<&str>,
    what_was_done: Option<&str>,
    key_decisions: &[String],
) -> String {
    let mut content = String::new();
    content.push_str("# Large Output Spillover\n\n");
    content.push_str(&format!(
        "Combined report text ({combined_bytes} bytes) exceeded the configured threshold ({threshold_bytes} bytes).\n\n"
    ));
    if let Some(summary) = summary {
        content.push_str("## Summary\n\n");
        content.push_str(summary);
        content.push_str("\n\n");
    }
    if let Some(what_was_done) = what_was_done {
        content.push_str("## What Was Done\n\n");
        content.push_str(what_was_done);
        content.push_str("\n\n");
    }
    if !key_decisions.is_empty() {
        content.push_str("## Key Decisions\n\n");
        for decision in key_decisions {
            content.push_str("- ");
            content.push_str(decision);
            content.push('\n');
        }
    }
    content
}

fn truncate_with_ellipsis(input: &str, max_chars: usize) -> String {
    let total_chars = input.chars().count();
    if total_chars <= max_chars {
        return input.to_string();
    }
    let take_chars = max_chars.saturating_sub(3);
    let mut truncated = input.chars().take(take_chars).collect::<String>();
    truncated.push_str("...");
    truncated
}

fn upsert_session_artifact(
    result: &mut SessionResult,
    artifact_path: &str,
    size_bytes: Option<u64>,
) {
    if let Some(existing) = result
        .artifacts
        .iter_mut()
        .find(|artifact| artifact.path == artifact_path)
    {
        existing.size_bytes = size_bytes;
        return;
    }
    result.artifacts.push(SessionArtifact {
        path: artifact_path.to_string(),
        line_count: None,
        size_bytes,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::tempdir;

    #[test]
    fn manager_result_redaction_preserves_toml_datetime_values() {
        let sidecar = toml::toml! {
            started_at = 2026-04-19T12:34:56Z
            [auth]
            token = "secret-token"
        }
        .into();

        let redacted = redact_result_sidecar_value(&sidecar).expect("redacted sidecar");

        assert!(matches!(
            redacted.get("started_at"),
            Some(toml::Value::Datetime(_))
        ));
        assert_eq!(
            redacted
                .get("auth")
                .and_then(toml::Value::as_table)
                .and_then(|table| table.get("token")),
            Some(&toml::Value::String("[REDACTED]".to_string()))
        );
    }

    #[test]
    fn manager_result_redaction_preserves_nested_json_string_redaction() {
        let sidecar = toml::toml! {
            payload = "{\"secret\":\"top-secret\"}"
        }
        .into();

        let redacted = redact_result_sidecar_value(&sidecar).expect("redacted sidecar");
        let payload = redacted
            .get("payload")
            .and_then(toml::Value::as_str)
            .expect("payload string");

        assert!(!payload.contains("top-secret"));
        assert!(payload.contains("[REDACTED]"));
    }

    #[test]
    fn load_result_in_ignores_orphaned_sidecar() {
        let td = tempdir().expect("tempdir");
        let session_id = crate::validate::new_session_id();
        let session_dir = super::super::super::get_session_dir_in(td.path(), &session_id);
        std::fs::create_dir_all(session_dir.join("output")).expect("create output dir");
        let result_path = session_dir.join(RESULT_FILE_NAME);

        let now = Utc::now();
        let turn_1_result = SessionResult {
            status: "success".to_string(),
            exit_code: 0,
            summary: "turn 1".to_string(),
            tool: "codex".to_string(),
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
            started_at: now,
            completed_at: now,
            events_count: 1,
            artifacts: vec![SessionArtifact::new("output/acp-events.jsonl")],
            peak_memory_mb: None,
            manager_fields: crate::result::SessionManagerFields {
                report: Some(
                    toml::toml! {
                        [repo_write_audit]
                        added = ["turn-1.txt"]
                    }
                    .into(),
                ),
                ..Default::default()
            },
        };
        save_result_in(
            td.path(),
            &session_id,
            &turn_1_result,
            SaveOptions::default(),
        )
        .expect("save turn 1");

        let turn_2_result = SessionResult {
            summary: "turn 2".to_string(),
            manager_fields: Default::default(),
            ..turn_1_result
        };
        save_result_in(
            td.path(),
            &session_id,
            &turn_2_result,
            SaveOptions::default(),
        )
        .expect("save turn 2");

        let mut turn_2_envelope: SessionResult =
            toml::from_str(&std::fs::read_to_string(&result_path).expect("read turn 2 envelope"))
                .expect("parse turn 2 envelope");
        turn_2_envelope
            .artifacts
            .retain(|artifact| artifact.path != CONTRACT_RESULT_ARTIFACT_PATH);
        let contents =
            toml::to_string_pretty(&turn_2_envelope).expect("serialize orphaned envelope");
        write_file_atomically(&result_path, &contents).expect("persist orphaned envelope");

        let reloaded = load_result_in(td.path(), &session_id)
            .expect("load result")
            .expect("result should exist");
        assert!(
            reloaded
                .artifacts
                .iter()
                .all(|artifact| artifact.path != CONTRACT_RESULT_ARTIFACT_PATH),
            "turn 2 envelope must not advertise the manager sidecar"
        );
        assert!(
            reloaded.manager_fields.as_sidecar().is_none(),
            "orphaned sidecar must not leak prior manager fields into turn 2"
        );
    }

    #[test]
    fn save_result_in_spills_large_existing_manager_report_to_artifact() {
        let td = tempdir().expect("tempdir");
        let state =
            super::super::super::create_session_in(td.path(), td.path(), None, None, Some("codex"))
                .expect("create session");
        let session_dir =
            super::super::super::get_session_dir_in(td.path(), &state.meta_session_id);

        let long_summary = "summary ".repeat(40);
        let long_body = "implemented detailed steps ".repeat(30);
        let long_decisions = vec![
            "decision A ".repeat(20),
            "decision B ".repeat(20),
            "decision C ".repeat(20),
            "decision D ".repeat(20),
        ];
        let sidecar = toml::Value::Table(
            [
                (
                    "report".to_string(),
                    toml::Value::Table(
                        [
                            (
                                "summary".to_string(),
                                toml::Value::String(long_summary.clone()),
                            ),
                            (
                                "what_was_done".to_string(),
                                toml::Value::String(long_body.clone()),
                            ),
                            (
                                "key_decisions".to_string(),
                                toml::Value::Array(
                                    long_decisions
                                        .iter()
                                        .cloned()
                                        .map(toml::Value::String)
                                        .collect(),
                                ),
                            ),
                        ]
                        .into_iter()
                        .collect(),
                    ),
                ),
                (
                    "artifacts".to_string(),
                    toml::Value::Table(
                        [(
                            "commit_hash".to_string(),
                            toml::Value::String("abc1234".to_string()),
                        )]
                        .into_iter()
                        .collect(),
                    ),
                ),
            ]
            .into_iter()
            .collect(),
        );
        std::fs::write(
            session_dir.join(CONTRACT_RESULT_ARTIFACT_PATH),
            toml::to_string_pretty(&sidecar).expect("serialize sidecar"),
        )
        .expect("write sidecar");

        let now = Utc::now();
        let runtime_result = SessionResult {
            status: "success".to_string(),
            exit_code: 0,
            summary: "runtime summary".to_string(),
            tool: "codex".to_string(),
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
            started_at: now,
            completed_at: now,
            events_count: 1,
            artifacts: vec![SessionArtifact::new("output/acp-events.jsonl")],
            peak_memory_mb: None,
            manager_fields: Default::default(),
        };
        save_result_in_with_threshold(
            td.path(),
            &state.meta_session_id,
            &runtime_result,
            SaveOptions::default(),
            256,
        )
        .expect("save result");

        let reloaded = load_result_in(td.path(), &state.meta_session_id)
            .expect("load result")
            .expect("result should exist");
        assert!(
            reloaded
                .artifacts
                .iter()
                .any(|artifact| artifact.path == CONTRACT_RESULT_ARTIFACT_PATH)
        );
        assert!(
            reloaded
                .artifacts
                .iter()
                .any(|artifact| artifact.path == LARGE_OUTPUT_ARTIFACT_PATH),
            "runtime envelope should advertise the spillover artifact"
        );

        let spilled_sidecar =
            load_optional_result_sidecar(&session_dir, CONTRACT_RESULT_ARTIFACT_PATH)
                .expect("load sidecar")
                .expect("sidecar should exist");
        assert_eq!(
            spilled_sidecar
                .get("artifacts")
                .and_then(|value| value.get("commit_hash"))
                .and_then(toml::Value::as_str),
            Some("abc1234")
        );
        assert_eq!(
            spilled_sidecar
                .get("artifacts")
                .and_then(|value| value.get("large_output_path"))
                .and_then(toml::Value::as_str),
            Some("$CSA_SESSION_DIR/artifacts/large-output-report.md")
        );
        assert!(
            spilled_sidecar
                .get("artifacts")
                .and_then(|value| value.get("reverse_prompt"))
                .and_then(toml::Value::as_str)
                .is_some_and(|value| value.contains("head -100")),
            "reverse_prompt should guide selective artifact reads"
        );
        assert!(
            spilled_sidecar
                .get("report")
                .and_then(|value| value.get("summary"))
                .and_then(toml::Value::as_str)
                .is_some_and(|value| {
                    value.contains(
                        "[full report: $CSA_SESSION_DIR/artifacts/large-output-report.md]",
                    )
                })
        );

        let artifact_contents =
            std::fs::read_to_string(session_dir.join(LARGE_OUTPUT_ARTIFACT_PATH))
                .expect("read spillover artifact");
        assert!(artifact_contents.contains(&long_summary));
        assert!(artifact_contents.contains(&long_body));
        assert!(artifact_contents.contains(&long_decisions[3]));
    }

    #[test]
    fn resolve_report_spill_threshold_uses_project_config_override() {
        let td = tempdir().expect("tempdir");
        let config_dir = td.path().join(".csa");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(
            config_dir.join("config.toml"),
            "schema_version = 1\n[session]\nresult_report_spill_threshold_bytes = 2048\n",
        )
        .expect("write config");

        assert_eq!(resolve_report_spill_threshold_bytes(td.path()), 2048);
    }
}
