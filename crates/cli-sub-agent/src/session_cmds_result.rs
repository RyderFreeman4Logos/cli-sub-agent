use anyhow::Result;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;

use csa_session::SessionResultView;
use csa_session::state::ReviewSessionMeta;

use crate::session_cmds::{
    ensure_terminal_result_for_dead_active_session, resolve_session_prefix_with_global_fallback,
};

#[path = "session_cmds_result_artifacts.rs"]
mod artifacts;
#[path = "session_cmds_result_display.rs"]
mod display;
#[path = "session_cmds_result_tool_output.rs"]
mod tool_output;

use display::{
    display_pre_exec_summary_if_present, display_result_json, display_result_text,
    display_structured_output, load_total_token_usage,
};

#[derive(Debug, Clone)]
struct TranscriptSummary {
    event_count: u64,
    size_bytes: u64,
    first_timestamp: Option<String>,
    last_timestamp: Option<String>,
}

fn load_transcript_summary(session_dir: &Path) -> Result<Option<TranscriptSummary>> {
    let transcript_path = session_dir.join("output").join("acp-events.jsonl");
    if !transcript_path.is_file() {
        return Ok(None);
    }

    let size_bytes = fs::metadata(&transcript_path)?.len();
    let file = File::open(&transcript_path)?;
    let reader = BufReader::new(file);

    let mut event_count = 0u64;
    let mut first_timestamp: Option<String> = None;
    let mut last_timestamp: Option<String> = None;

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        event_count = event_count.saturating_add(1);
        if let Some(ts) = extract_transcript_timestamp(&line) {
            if first_timestamp.is_none() {
                first_timestamp = Some(ts.clone());
            }
            last_timestamp = Some(ts);
        }
    }

    Ok(Some(TranscriptSummary {
        event_count,
        size_bytes,
        first_timestamp,
        last_timestamp,
    }))
}

fn extract_transcript_timestamp(line: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()?
        .get("ts")?
        .as_str()
        .map(ToString::to_string)
}

/// Options for structured output display in `csa session result`.
#[derive(Debug, Default)]
pub(crate) struct StructuredOutputOpts {
    pub summary: bool,
    pub section: Option<String>,
    pub full: bool,
}

impl StructuredOutputOpts {
    fn is_active(&self) -> bool {
        self.summary || self.section.is_some() || self.full
    }
}

pub(crate) fn handle_session_result(
    session: String,
    json: bool,
    cd: Option<String>,
    structured: StructuredOutputOpts,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_global_fallback(&project_root, &session)?;
    let wrapper_id = resolved.session_id.clone();
    let wrapper_session_dir = resolved.sessions_dir.join(&wrapper_id);
    let mut resolved_id = wrapper_id.clone();
    let mut session_dir = wrapper_session_dir.clone();

    // Use the foreign project root for cross-project sessions, local otherwise.
    let effective_root = resolved
        .foreign_project_root
        .as_deref()
        .unwrap_or(&project_root);
    let is_cross_project = resolved.foreign_project_root.is_some();

    let registry_state_loss = wrapper_session_dir.is_dir()
        && csa_session::load_session(effective_root, &wrapper_id).is_err();
    if registry_state_loss
        && structured.summary
        && display_pre_exec_summary_if_present(&wrapper_session_dir, json)?
    {
        return Ok(());
    }

    if registry_state_loss
        && !registry_loss_display_artifact_exists(&wrapper_session_dir, &structured)?
        && crate::session_observability::emit_session_registry_state_loss_diagnostic(
            effective_root,
            &wrapper_id,
            &wrapper_session_dir,
        )
    {
        return Ok(());
    }

    let resume_target = csa_session::resolve_resume_target_from_dir(effective_root, &session_dir)?;
    let follows_resume_target = resume_target.is_some();
    if let Some(target) = resume_target {
        resolved_id = target.session_id;
        session_dir = target.session_dir;
    }

    let daemon_completion_result =
        match crate::session_cmds_daemon::finalize_daemon_completion_if_present(&session_dir) {
            Ok(result) => result,
            Err(err) => {
                tracing::warn!(
                    session_id = %resolved_id,
                    error = %err,
                    "Failed to finalize daemon completion packet in session result"
                );
                None
            }
        };

    let handoff_blocks_target_reconcile = follows_resume_target
        && crate::session_resume_handoff::resume_handoff_blocks_target_reconcile(
            &wrapper_session_dir,
            &session_dir,
        );
    if !handoff_blocks_target_reconcile
        && let Err(err) = ensure_terminal_result_for_dead_active_session(
            effective_root,
            &resolved_id,
            "session result",
        )
    {
        tracing::warn!(
            session_id = %resolved_id,
            error = %err,
            "Failed to reconcile dead Active session in session result"
        );
    } else if handoff_blocks_target_reconcile {
        tracing::debug!(
            wrapper_session_id = %wrapper_id,
            target_session_id = %resolved_id,
            "resume wrapper still owns target handoff; skipping target reconciliation in session result"
        );
    }

    let repaired_result = if is_cross_project || registry_state_loss {
        match crate::session_observability::refresh_and_repair_result_from_dir(&session_dir) {
            Ok(result) => result,
            Err(err) => {
                tracing::warn!(
                    session_id = %resolved_id,
                    error = %err,
                    "Failed to refresh cross-project session result"
                );
                None
            }
        }
    } else {
        match crate::session_observability::refresh_and_repair_result(&project_root, &resolved_id) {
            Ok(result) => result,
            Err(err) => {
                tracing::warn!(
                    session_id = %resolved_id,
                    error = %err,
                    "Failed to refresh session result contract in session result"
                );
                None
            }
        }
    };
    let repaired_result = repaired_result.or(daemon_completion_result);

    if let Some(result) = repaired_result.as_ref().filter(|result| {
        crate::session_tier_failover::is_pending_tier_failover_handoff(&session_dir, result)
    }) {
        crate::session_tier_failover::emit_pending_tier_failover_handoff(
            &resolved_id,
            result,
            json,
        );
        return Ok(());
    }
    if structured.is_active() {
        return display_structured_output(&session_dir, &resolved_id, &structured, json);
    }

    let transcript_summary = match load_transcript_summary(&session_dir) {
        Ok(summary) => summary,
        Err(err) => {
            tracing::warn!(
                session_id = %resolved_id,
                path = %session_dir.display(),
                error = %err,
                "Failed to load transcript summary; continuing without transcript metadata"
            );
            None
        }
    };
    let review_meta = match load_review_meta(&session_dir) {
        Ok(meta) => meta,
        Err(err) => {
            tracing::warn!(
                session_id = %resolved_id,
                path = %session_dir.display(),
                error = %err,
                "Failed to load review metadata; continuing without review_meta"
            );
            None
        }
    };
    match repaired_result {
        Some(result) => {
            let result_view = match csa_session::load_result_view(effective_root, &resolved_id) {
                Ok(Some(view)) => view,
                Ok(None) => SessionResultView {
                    envelope: result.clone(),
                    manager_sidecar: result.manager_fields.as_sidecar(),
                    legacy_sidecar: None,
                },
                Err(err) => {
                    tracing::warn!(
                        session_id = %resolved_id,
                        error = %err,
                        "Failed to load result sidecars; continuing with runtime envelope only"
                    );
                    SessionResultView {
                        envelope: result.clone(),
                        manager_sidecar: result.manager_fields.as_sidecar(),
                        legacy_sidecar: None,
                    }
                }
            };
            // Cross-project sessions cannot resolve their state through the
            // local project path; load directly from the session_dir state.toml.
            let token_usage = load_total_token_usage(&session_dir);
            if json {
                display_result_json(
                    &result_view,
                    transcript_summary.as_ref(),
                    review_meta.as_ref(),
                    token_usage.as_ref(),
                )?;
            } else {
                display_result_text(
                    &resolved_id,
                    &session_dir,
                    &result_view,
                    transcript_summary.as_ref(),
                    review_meta.as_ref(),
                    token_usage.as_ref(),
                );
            }
        }
        None => {
            // For cross-project sessions, skip phase lookup (would fail).
            let phase_label = if is_cross_project {
                None
            } else {
                csa_session::load_session(&project_root, &resolved_id)
                    .ok()
                    .map(|session| session.phase.to_string())
            };
            eprintln!(
                "{}",
                crate::session_observability::build_missing_result_diagnostic(
                    &resolved_id,
                    &session_dir,
                    phase_label.as_deref(),
                )
            );
        }
    }
    Ok(())
}

fn registry_loss_display_artifact_exists(
    session_dir: &Path,
    structured: &StructuredOutputOpts,
) -> Result<bool> {
    if session_dir
        .join(csa_session::result::RESULT_FILE_NAME)
        .is_file()
    {
        return Ok(true);
    }

    requested_structured_output_exists(session_dir, structured)
}

fn requested_structured_output_exists(
    session_dir: &Path,
    structured: &StructuredOutputOpts,
) -> Result<bool> {
    if !structured.is_active() {
        return Ok(false);
    }

    let output_log = session_dir.join("output.log");
    let output_log_non_empty = output_log
        .metadata()
        .is_ok_and(|metadata| metadata.len() > 0);

    if structured.summary {
        return Ok(output_index_has_section(session_dir, "summary")?
            || output_index_has_section(session_dir, "full")?
            || output_log_non_empty);
    }

    if let Some(section_id) = structured.section.as_deref() {
        return output_index_has_section(session_dir, section_id);
    }

    if structured.full {
        return Ok(output_index_has_any_section(session_dir)? || output_log_non_empty);
    }

    Ok(false)
}

fn output_index_has_section(session_dir: &Path, section_id: &str) -> Result<bool> {
    Ok(
        csa_session::load_output_index(session_dir)?.is_some_and(|index| {
            index
                .sections
                .iter()
                .any(|section| section.id == section_id)
        }),
    )
}

fn output_index_has_any_section(session_dir: &Path) -> Result<bool> {
    Ok(
        csa_session::load_output_index(session_dir)?
            .is_some_and(|index| !index.sections.is_empty()),
    )
}

fn load_review_meta(session_dir: &Path) -> Result<Option<ReviewSessionMeta>> {
    let review_meta_path = session_dir.join("review_meta.json");
    if !review_meta_path.is_file() {
        return Ok(None);
    }

    let content = fs::read_to_string(&review_meta_path)?;
    let review_meta = serde_json::from_str(&content)?;
    Ok(Some(review_meta))
}

pub(crate) use crate::session_cmds_result_measure::handle_session_measure;
#[cfg(test)]
pub(crate) use crate::session_cmds_result_measure::{compute_token_measurement, format_number};
pub(crate) use artifacts::handle_session_artifacts;
#[cfg(test)]
use display::{
    build_all_sections_json_payload, build_gate_aware_summary_content, build_result_json_payload,
    build_summary_section_json_payload, display_all_sections, display_single_section,
    display_summary_section, gate_summary_employee_section, load_structured_post_exec_gate_report,
    render_result_sidecar_for_text, render_token_usage_lines, structured_sections_with_gate_first,
};
pub(crate) use tool_output::handle_session_tool_output;

#[cfg(test)]
#[path = "session_cmds_result_post_exec_gate_tests.rs"]
mod post_exec_gate_tests;
#[cfg(test)]
#[path = "session_cmds_result_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "session_cmds_result_tier_failover_tests.rs"]
mod tier_failover_tests;
#[cfg(test)]
#[path = "session_cmds_result_token_tests.rs"]
mod token_tests;
