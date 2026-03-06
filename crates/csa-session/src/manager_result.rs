use crate::result::{RESULT_FILE_NAME, SessionArtifact, SessionResult};
use crate::validate::validate_session_id;
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::Path;

const TRANSCRIPT_FILE_NAME: &str = "acp-events.jsonl";
const USER_RESULT_FILE_NAME: &str = "user-result.toml";
const USER_RESULT_ARTIFACT_PATH: &str = "output/user-result.toml";
const RUNTIME_RESULT_KEYS: [&str; 8] = [
    "status",
    "exit_code",
    "summary",
    "tool",
    "started_at",
    "completed_at",
    "events_count",
    "artifacts",
];

/// Write a session result to disk
pub fn save_result(project_path: &Path, session_id: &str, result: &SessionResult) -> Result<()> {
    let base_dir = super::resolve_write_base_dir(project_path, session_id)?;
    save_result_in(&base_dir, session_id, result)
}

pub(crate) fn save_result_in(
    base_dir: &Path,
    session_id: &str,
    result: &SessionResult,
) -> Result<()> {
    validate_session_id(session_id)?;
    let session_dir = super::get_session_dir_in(base_dir, session_id);
    let result_path = session_dir.join(RESULT_FILE_NAME);

    let mut existing_table = None;
    let mut existing_contents = None;
    let mut has_custom_schema = false;
    if result_path.exists() {
        let contents = fs::read_to_string(&result_path).with_context(|| {
            format!("Failed to read existing result: {}", result_path.display())
        })?;
        match toml::from_str::<toml::Value>(&contents) {
            Ok(toml::Value::Table(table)) => {
                has_custom_schema = table_has_custom_schema(&table);
                existing_table = Some(table);
            }
            Ok(_) | Err(_) => {
                // Preserve malformed/non-table user result in sidecar before overwriting.
                has_custom_schema = true;
            }
        }
        existing_contents = Some(contents);
    }

    let mut persisted_result = result.clone();
    if has_custom_schema {
        let Some(contents) = existing_contents.as_deref() else {
            bail!("Expected existing result content when custom schema was detected");
        };
        preserve_user_result_snapshot(&session_dir, contents)?;
    }
    retain_user_result_artifact_if_snapshot_exists(&session_dir, &mut persisted_result)?;

    let runtime_table = session_result_to_table(&persisted_result)?;
    let mut merged_table = existing_table.unwrap_or_default();
    for key in RUNTIME_RESULT_KEYS {
        merged_table.remove(key);
    }
    merged_table.extend(runtime_table);
    let contents = toml::to_string_pretty(&toml::Value::Table(merged_table))
        .context("Failed to serialize session result")?;
    fs::write(&result_path, contents)
        .with_context(|| format!("Failed to write result: {}", result_path.display()))?;
    Ok(())
}

fn preserve_user_result_snapshot(session_dir: &Path, contents: &str) -> Result<()> {
    let output_dir = session_dir.join("output");
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("Failed to create output dir: {}", output_dir.display()))?;
    let snapshot_path = output_dir.join(USER_RESULT_FILE_NAME);
    if snapshot_path.exists() {
        if snapshot_path.is_file() {
            return Ok(());
        }
        bail!(
            "User result snapshot path exists but is not a file: {}",
            snapshot_path.display()
        );
    }
    fs::write(&snapshot_path, contents).with_context(|| {
        format!(
            "Failed to write user result snapshot: {}",
            snapshot_path.display()
        )
    })
}

fn retain_user_result_artifact_if_snapshot_exists(
    session_dir: &Path,
    result: &mut SessionResult,
) -> Result<()> {
    let snapshot_path = session_dir.join(USER_RESULT_ARTIFACT_PATH);
    if !snapshot_path.exists() {
        return Ok(());
    }
    if !snapshot_path.is_file() {
        bail!(
            "User result snapshot path exists but is not a file: {}",
            snapshot_path.display()
        );
    }
    ensure_user_result_artifact(result);
    Ok(())
}

fn ensure_user_result_artifact(result: &mut SessionResult) {
    if result
        .artifacts
        .iter()
        .any(|artifact| artifact.path == USER_RESULT_ARTIFACT_PATH)
    {
        return;
    }
    result
        .artifacts
        .push(SessionArtifact::new(USER_RESULT_ARTIFACT_PATH));
}

fn session_result_to_table(result: &SessionResult) -> Result<toml::Table> {
    let value =
        toml::Value::try_from(result).context("Failed to convert session result to TOML value")?;
    let Some(table) = value.as_table() else {
        bail!("Session result must serialize to a TOML table");
    };
    Ok(table.clone())
}

fn table_has_custom_schema(table: &toml::Table) -> bool {
    table
        .iter()
        .any(|(key, value)| !value_matches_runtime_schema(key, value))
}

fn value_matches_runtime_schema(key: &str, value: &toml::Value) -> bool {
    match key {
        "status" | "summary" | "tool" | "started_at" | "completed_at" => value.is_str(),
        "exit_code" | "events_count" => value.is_integer(),
        "artifacts" => artifacts_value_matches_runtime_schema(value),
        _ => false,
    }
}

fn artifacts_value_matches_runtime_schema(value: &toml::Value) -> bool {
    let Some(entries) = value.as_array() else {
        return false;
    };

    entries.iter().all(|entry| match entry {
        toml::Value::String(_) => true,
        toml::Value::Table(table) => {
            let Some(path) = table.get("path") else {
                return false;
            };
            if !path.is_str() {
                return false;
            }

            table.iter().all(|(key, value)| match key.as_str() {
                "path" => value.is_str(),
                "line_count" | "size_bytes" => {
                    value.as_integer().map(|num| num >= 0).unwrap_or(false)
                }
                _ => false,
            })
        }
        _ => false,
    })
}

/// Load a session result
pub fn load_result(project_path: &Path, session_id: &str) -> Result<Option<SessionResult>> {
    let base_dir = super::resolve_read_base_dir(project_path, Some(session_id))?;
    load_result_in(&base_dir, session_id)
}

pub(crate) fn load_result_in(base_dir: &Path, session_id: &str) -> Result<Option<SessionResult>> {
    validate_session_id(session_id)?;
    let session_dir = super::get_session_dir_in(base_dir, session_id);
    let result_path = session_dir.join(RESULT_FILE_NAME);
    if !result_path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(&result_path)
        .with_context(|| format!("Failed to read result: {}", result_path.display()))?;
    let result: SessionResult = toml::from_str(&contents)
        .with_context(|| format!("Failed to parse result: {}", result_path.display()))?;
    Ok(Some(result))
}

/// List artifacts in a session's output/ directory
pub fn list_artifacts(project_path: &Path, session_id: &str) -> Result<Vec<String>> {
    let base_dir = super::resolve_read_base_dir(project_path, Some(session_id))?;
    list_artifacts_in(&base_dir, session_id)
}

pub(crate) fn list_artifacts_in(base_dir: &Path, session_id: &str) -> Result<Vec<String>> {
    validate_session_id(session_id)?;
    let session_dir = super::get_session_dir_in(base_dir, session_id);
    let output_dir = session_dir.join("output");
    if !output_dir.exists() {
        return Ok(Vec::new());
    }
    let mut artifacts = Vec::new();
    for entry in fs::read_dir(&output_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            artifacts.push(entry.file_name().to_string_lossy().to_string());
        }
    }
    let transcript_path = output_dir.join(TRANSCRIPT_FILE_NAME);
    if transcript_path.is_file() && !artifacts.iter().any(|name| name == TRANSCRIPT_FILE_NAME) {
        artifacts.push(TRANSCRIPT_FILE_NAME.to_string());
    }
    artifacts.sort();
    Ok(artifacts)
}
