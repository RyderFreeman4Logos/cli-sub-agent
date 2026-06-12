use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::Utc;

use super::super::session_has_terminal_process;

pub(super) fn suppress_pending_tier_failover_result(
    session_id: &str,
    session_dir: &Path,
    result: csa_session::SessionResult,
) -> Option<csa_session::SessionResult> {
    if crate::session_tier_failover::is_pending_tier_failover_handoff(session_dir, &result) {
        tracing::debug!(
            session_id,
            status = %result.status,
            "Ignoring intermediate tier-failover result while fallback handoff is still live"
        );
        None
    } else {
        Some(result)
    }
}

fn load_completed_daemon_result(
    project_root: &Path,
    session_id: &str,
    session_dir: &Path,
) -> Result<Option<csa_session::SessionResult>> {
    let daemon_alive_at_refresh_start = session_has_terminal_process(session_dir);
    let result =
        match crate::session_observability::refresh_and_repair_result(project_root, session_id) {
            Ok(Some(result)) => result,
            Ok(None) => return Ok(None),
            Err(err) if daemon_alive_at_refresh_start => {
                tracing::debug!(
                    session_id,
                    error = %err,
                    "Ignoring transient result refresh failure while daemon is still alive"
                );
                return Ok(None);
            }
            Err(err) => return Err(err),
        };

    Ok(suppress_pending_tier_failover_result(
        session_id,
        session_dir,
        result,
    ))
}

/// Refresh result via session_dir for cross-project sessions or via project_root otherwise.
pub(super) fn refresh_result_for_wait(
    project_root: &Path,
    session_id: &str,
    session_dir: &Path,
    is_cross_project: bool,
) -> Result<Option<csa_session::SessionResult>> {
    let result = if is_cross_project {
        crate::session_observability::refresh_and_repair_result_from_dir(session_dir)
    } else {
        crate::session_observability::refresh_and_repair_result(project_root, session_id)
    }?;
    Ok(result
        .and_then(|result| suppress_pending_tier_failover_result(session_id, session_dir, result)))
}

fn load_completed_daemon_result_adaptive(
    project_root: &Path,
    session_id: &str,
    session_dir: &Path,
    is_cross_project: bool,
) -> Result<Option<csa_session::SessionResult>> {
    if is_cross_project {
        let daemon_alive_at_refresh_start = session_has_terminal_process(session_dir);
        let result = match crate::session_observability::refresh_and_repair_result_from_dir(
            session_dir,
        ) {
            Ok(Some(result)) => result,
            Ok(None) => return Ok(None),
            Err(err) if daemon_alive_at_refresh_start => {
                tracing::debug!(
                    session_id,
                    error = %err,
                    "Ignoring transient result refresh failure (cross-project) while daemon is still alive"
                );
                return Ok(None);
            }
            Err(err) => return Err(err),
        };
        Ok(suppress_pending_tier_failover_result(
            session_id,
            session_dir,
            result,
        ))
    } else {
        load_completed_daemon_result(project_root, session_id, session_dir)
    }
}

fn load_output_result_fallback(
    session_id: &str,
    session_dir: &Path,
) -> Result<Option<csa_session::SessionResult>> {
    let Some(output_result_artifact_path) =
        expected_in_flight_turn_result_artifact_path(session_dir)
            .or_else(|| current_legacy_result_artifact_path(session_dir))
    else {
        return Ok(None);
    };
    let output_result_path = session_dir.join(output_result_artifact_path);

    tracing::debug!(
        path = %output_result_path.display(),
        "Found manager result artifact as fallback completion signal"
    );

    let contents = fs::read_to_string(&output_result_path)?;
    let result = parse_output_result_artifact(&contents).with_context(|| {
        format!(
            "Failed to parse manager result artifact fallback: {}",
            output_result_path.display()
        )
    })?;
    Ok(suppress_pending_tier_failover_result(
        session_id,
        session_dir,
        result,
    ))
}

fn parse_output_result_artifact(contents: &str) -> Result<csa_session::SessionResult> {
    match toml::from_str::<csa_session::SessionResult>(contents) {
        Ok(result) => Ok(result),
        Err(flat_schema_error) => parse_nested_manager_result_artifact(contents)
            .with_context(|| {
                format!(
                    "artifact is neither a flat SessionResult nor a canonical nested [result] manager sidecar: {flat_schema_error}"
                )
            }),
    }
}

#[cfg(test)]
pub(crate) fn parse_output_result_artifact_for_test(
    contents: &str,
) -> Result<csa_session::SessionResult> {
    parse_output_result_artifact(contents)
}

fn parse_nested_manager_result_artifact(contents: &str) -> Result<csa_session::SessionResult> {
    let value: toml::Value =
        toml::from_str(contents).context("manager result artifact is not valid TOML")?;
    let Some(table) = value.as_table() else {
        bail!("manager result artifact must be a TOML table");
    };
    let Some(result_table) = table.get("result").and_then(toml::Value::as_table) else {
        bail!("manager result artifact must contain a [result] table");
    };
    let status = required_nonempty_string(result_table, "status")?;
    let summary = optional_string(result_table, "summary")?
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("manager result sidecar reported status={status}"));
    let exit_code =
        optional_i32(result_table, "exit_code")?.unwrap_or_else(|| status_exit_code(status));
    let tool = nested_tool_name(table).unwrap_or("unknown").to_string();
    let now = Utc::now();

    Ok(csa_session::SessionResult {
        post_exec_gate: None,
        status: status.to_string(),
        exit_code,
        summary,
        tool,
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: Vec::new(),
        manager_fields: csa_session::SessionManagerFields::from_sidecar(&value),
        ..Default::default()
    })
}

fn required_nonempty_string<'a>(table: &'a toml::Table, key: &str) -> Result<&'a str> {
    let Some(value) = table.get(key) else {
        bail!("manager [result].{key} is required");
    };
    let Some(text) = value.as_str() else {
        bail!("manager [result].{key} must be a string");
    };
    let text = text.trim();
    if text.is_empty() {
        bail!("manager [result].{key} must not be empty");
    }
    Ok(text)
}

fn optional_string<'a>(table: &'a toml::Table, key: &str) -> Result<Option<&'a str>> {
    let Some(value) = table.get(key) else {
        return Ok(None);
    };
    let Some(text) = value.as_str() else {
        bail!("manager [result].{key} must be a string when present");
    };
    Ok(Some(text))
}

fn optional_i32(table: &toml::Table, key: &str) -> Result<Option<i32>> {
    let Some(value) = table.get(key) else {
        return Ok(None);
    };
    let Some(exit_code) = value.as_integer() else {
        bail!("manager [result].{key} must be an integer when present");
    };
    let exit_code = i32::try_from(exit_code)
        .with_context(|| format!("manager [result].{key} is out of i32 range"))?;
    Ok(Some(exit_code))
}

fn nested_tool_name(table: &toml::Table) -> Option<&str> {
    table
        .get("tool")
        .and_then(toml::Value::as_table)
        .and_then(|tool| tool.get("name"))
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

fn status_exit_code(status: &str) -> i32 {
    if status.eq_ignore_ascii_case("success") {
        0
    } else {
        1
    }
}

fn expected_in_flight_turn_result_artifact_path(session_dir: &Path) -> Option<String> {
    let explicit_artifact_path = explicit_contract_result_artifact_path(session_dir);
    let marker_artifact_path = persisted_current_result_artifact_path(session_dir);
    let state_artifact_path = state_derived_next_turn_result_artifact_path(session_dir);

    if let Some(artifact_path) = explicit_artifact_path
        && (marker_artifact_path.as_deref() == Some(artifact_path.as_str())
            || state_artifact_path.as_deref() == Some(artifact_path.as_str()))
    {
        return Some(artifact_path);
    }
    if let Some(artifact_path) = marker_artifact_path {
        return Some(artifact_path);
    }

    state_artifact_path
}

#[cfg(test)]
pub(crate) fn expected_in_flight_turn_result_artifact_path_for_test(
    session_dir: &Path,
) -> Option<String> {
    expected_in_flight_turn_result_artifact_path(session_dir)
}

fn state_derived_next_turn_result_artifact_path(session_dir: &Path) -> Option<String> {
    let state_path = session_dir.join("state.toml");
    let contents = fs::read_to_string(state_path).ok()?;
    let table: toml::Table = toml::from_str(&contents).ok()?;
    let completed_turn_count = table
        .get("turn_count")?
        .as_integer()
        .and_then(|value| u32::try_from(value).ok())?;

    csa_session::existing_next_turn_contract_result_artifact_path(session_dir, completed_turn_count)
}

fn explicit_contract_result_artifact_path(session_dir: &Path) -> Option<String> {
    let contract_path = std::env::var_os(csa_session::RESULT_TOML_PATH_CONTRACT_ENV)?;
    valid_existing_manager_artifact_path(session_dir, PathBuf::from(contract_path))
}

fn persisted_current_result_artifact_path(session_dir: &Path) -> Option<String> {
    let artifact_path = persisted_current_result_artifact_candidate(session_dir)?;
    session_dir
        .join(&artifact_path)
        .is_file()
        .then_some(artifact_path)
}

fn persisted_current_result_artifact_candidate(session_dir: &Path) -> Option<String> {
    let marker_path =
        crate::pipeline::result_contract::current_result_artifact_marker_path(session_dir);
    let contents = fs::read_to_string(marker_path).ok()?;
    let table: toml::Table = toml::from_str(&contents).ok()?;
    let artifact_path = table.get("artifact_path")?.as_str()?;
    valid_manager_artifact_path(session_dir, PathBuf::from(artifact_path))
}

fn valid_existing_manager_artifact_path(
    session_dir: &Path,
    candidate_path: PathBuf,
) -> Option<String> {
    let artifact_path = valid_manager_artifact_path(session_dir, candidate_path)?;
    session_dir
        .join(&artifact_path)
        .is_file()
        .then_some(artifact_path)
}

fn valid_manager_artifact_path(session_dir: &Path, candidate_path: PathBuf) -> Option<String> {
    let artifact_path = if candidate_path.is_absolute() {
        candidate_path.strip_prefix(session_dir).ok()?.to_path_buf()
    } else {
        candidate_path
    };
    if artifact_path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    let artifact_path = artifact_path.to_string_lossy().replace('\\', "/");
    if !csa_session::is_manager_result_artifact_path(&artifact_path) {
        return None;
    }
    Some(artifact_path)
}

fn current_legacy_result_artifact_path(session_dir: &Path) -> Option<String> {
    let current_artifact = persisted_current_result_artifact_candidate(session_dir)?;
    if current_artifact == csa_session::CONTRACT_RESULT_ARTIFACT_PATH
        || session_dir.join(&current_artifact).is_file()
    {
        return None;
    }

    session_dir
        .join(csa_session::CONTRACT_RESULT_ARTIFACT_PATH)
        .is_file()
        .then(|| csa_session::CONTRACT_RESULT_ARTIFACT_PATH.to_string())
}

pub(super) fn load_completed_daemon_result_with_fallback(
    project_root: &Path,
    session_id: &str,
    session_dir: &Path,
    is_cross_project: bool,
) -> Result<Option<csa_session::SessionResult>> {
    if let Some(result) = load_completed_daemon_result_adaptive(
        project_root,
        session_id,
        session_dir,
        is_cross_project,
    )? {
        return Ok(Some(result));
    }

    if !session_has_terminal_process(session_dir)
        && let Some(output_result) = load_output_result_fallback(session_id, session_dir)?
    {
        tracing::info!(
            session_id,
            "Session completion detected via output/result.toml fallback"
        );
        return Ok(Some(output_result));
    }

    Ok(None)
}
