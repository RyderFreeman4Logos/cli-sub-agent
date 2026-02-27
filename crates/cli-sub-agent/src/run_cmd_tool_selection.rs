//! Tool selection, session selection, and failover helpers for `csa run`.
//!
//! Extracted from `run_cmd.rs` to keep module sizes manageable.

use std::path::Path;

use anyhow::Result;

use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use csa_session::{MetaSessionState, SessionPhase, resolve_session_prefix};

use crate::cli::ReturnTarget;

/// Resolve the `--last` flag to a concrete session ID.
///
/// Returns the most recently accessed session ID plus an optional warning
/// string when the selection is ambiguous (multiple active sessions).
pub(crate) fn resolve_last_session_selection(
    sessions: Vec<MetaSessionState>,
) -> Result<(String, Option<String>)> {
    if sessions.is_empty() {
        anyhow::bail!("No sessions found. Run a task first to create one.");
    }

    let mut sorted_sessions = sessions;
    sorted_sessions.sort_by(|a, b| b.last_accessed.cmp(&a.last_accessed));
    let selected_id = sorted_sessions[0].meta_session_id.clone();

    let active_sessions: Vec<&MetaSessionState> = sorted_sessions
        .iter()
        .filter(|session| session.phase == SessionPhase::Active)
        .collect();

    if active_sessions.len() <= 1 {
        return Ok((selected_id, None));
    }

    let mut warning_lines = vec![
        format!(
            "warning: `--last` is ambiguous in this project: found {} active sessions.",
            active_sessions.len()
        ),
        format!("Resuming most recently accessed session: {}", selected_id),
        "Active sessions (session_id | last_accessed):".to_string(),
    ];

    for session in active_sessions {
        warning_lines.push(format!(
            "  {} | {}",
            session.meta_session_id,
            session.last_accessed.to_rfc3339()
        ));
    }

    warning_lines.push("Use `--session <session-id>` to choose explicitly.".to_string());

    Ok((selected_id, Some(warning_lines.join("\n"))))
}

/// Filter enabled tools to those from a different model family than the parent.
pub(crate) fn resolve_heterogeneous_candidates(
    parent_tool: &ToolName,
    enabled_tools: &[ToolName],
) -> Vec<ToolName> {
    let parent_family = parent_tool.model_family();
    enabled_tools
        .iter()
        .copied()
        .filter(|tool| tool.model_family() != parent_family)
        .collect()
}

/// Pop the next untried heterogeneous tool from the candidate list.
pub(crate) fn take_next_runtime_fallback_tool(
    candidates: &mut Vec<ToolName>,
    current_tool: ToolName,
    tried_tools: &[String],
) -> Option<ToolName> {
    while let Some(candidate) = candidates.first().copied() {
        candidates.remove(0);
        if candidate == current_tool {
            continue;
        }
        if tried_tools.iter().any(|tried| tried == candidate.as_str()) {
            continue;
        }
        return Some(candidate);
    }
    None
}

/// Read the slot wait timeout from project config or fall back to the default.
pub(crate) fn resolve_slot_wait_timeout_seconds(config: Option<&ProjectConfig>) -> u64 {
    config
        .map(|cfg| cfg.resources.slot_wait_timeout_seconds)
        .unwrap_or(csa_config::ResourcesConfig::default().slot_wait_timeout_seconds)
}

/// Resolve a session prefix (short ID) to a full session ID.
pub(crate) fn resolve_session_reference(
    project_root: &Path,
    session_ref: &str,
) -> Result<String> {
    let sessions_dir = csa_session::get_session_root(project_root)?.join("sessions");
    resolve_session_prefix(&sessions_dir, session_ref)
}

/// Resolve the `--return-to` target to a concrete session ID.
pub(crate) fn resolve_return_target_session_id(
    return_target: &ReturnTarget,
    project_root: &Path,
    fork_source_ref: Option<&str>,
    parent_flag: Option<&str>,
) -> Result<Option<String>> {
    match return_target {
        ReturnTarget::Last => {
            let sessions = csa_session::list_sessions(project_root, None)?;
            let (selected_id, _) = resolve_last_session_selection(sessions)?;
            Ok(Some(selected_id))
        }
        ReturnTarget::SessionId(session_ref) => {
            let resolved = resolve_session_reference(project_root, session_ref)?;
            Ok(Some(resolved))
        }
        ReturnTarget::Auto => {
            let env_parent = std::env::var("CSA_SESSION_ID").ok();
            let candidate = fork_source_ref
                .map(ToOwned::to_owned)
                .or_else(|| parent_flag.map(ToOwned::to_owned))
                .or(env_parent);

            if let Some(session_ref) = candidate {
                let resolved = resolve_session_reference(project_root, &session_ref)?;
                Ok(Some(resolved))
            } else {
                Ok(None)
            }
        }
    }
}
