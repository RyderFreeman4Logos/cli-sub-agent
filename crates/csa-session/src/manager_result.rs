use crate::result::{RESULT_FILE_NAME, SessionArtifact, SessionResult};
use crate::validate::validate_session_id;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const TRANSCRIPT_FILE_NAME: &str = "acp-events.jsonl";
const USER_RESULT_FILE_NAME: &str = "user-result.toml";
pub const RESULT_TOML_PATH_CONTRACT_ENV: &str = "CSA_RESULT_TOML_PATH_CONTRACT";
pub const CONTRACT_RESULT_ARTIFACT_PATH: &str = "output/result.toml";
pub const LEGACY_USER_RESULT_ARTIFACT_PATH: &str = "output/user-result.toml";
const RUNTIME_RESULT_KEYS: [&str; 11] = [
    "status",
    "exit_code",
    "summary",
    "tool",
    "original_tool",
    "fallback_tool",
    "fallback_reason",
    "started_at",
    "completed_at",
    "events_count",
    "artifacts",
];

#[path = "manager_result_spillover.rs"]
mod manager_result_spillover;

#[derive(Debug, Clone, Copy, Default)]
pub struct SaveOptions {
    /// When true, an empty manager_fields save deletes any existing
    /// manager sidecar. The default preserves previously persisted sidecars.
    pub clear_stale_manager_sidecar: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionResultView {
    pub envelope: SessionResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manager_sidecar: Option<toml::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legacy_sidecar: Option<toml::Value>,
}

pub fn render_redacted_result_sidecar(sidecar: &toml::Value) -> Result<String> {
    let redacted = redact_result_sidecar_value(sidecar)?;
    toml::to_string_pretty(&redacted).context("Failed to render redacted result sidecar")
}

pub fn redact_result_sidecar_value(sidecar: &toml::Value) -> Result<toml::Value> {
    let mut redacted = sidecar.clone();
    redact_toml_value(&mut redacted, None)?;
    Ok(redacted)
}

/// Write a session result to disk
pub fn save_result(project_path: &Path, session_id: &str, result: &SessionResult) -> Result<()> {
    save_result_with_options(project_path, session_id, result, SaveOptions::default())
}

/// Write a session result to disk with explicit sidecar preservation semantics.
pub fn save_result_with_options(
    project_path: &Path,
    session_id: &str,
    result: &SessionResult,
    options: SaveOptions,
) -> Result<()> {
    let base_dir = super::resolve_write_base_dir(project_path, session_id)?;
    let spill_threshold_bytes =
        manager_result_spillover::resolve_report_spill_threshold_bytes(project_path);
    save_result_in_with_threshold(
        &base_dir,
        session_id,
        result,
        options,
        spill_threshold_bytes,
    )
}

#[cfg(test)]
pub(crate) fn save_result_in(
    base_dir: &Path,
    session_id: &str,
    result: &SessionResult,
    options: SaveOptions,
) -> Result<()> {
    save_result_in_with_threshold(
        base_dir,
        session_id,
        result,
        options,
        csa_config::DEFAULT_RESULT_REPORT_SPILL_THRESHOLD_BYTES,
    )
}

fn save_result_in_with_threshold(
    base_dir: &Path,
    session_id: &str,
    result: &SessionResult,
    options: SaveOptions,
    spill_threshold_bytes: u64,
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
    let requested_manager_sidecar = persisted_result.manager_fields.as_sidecar();
    let preserve_existing_manager_sidecar = requested_manager_sidecar.is_some()
        || (!options.clear_stale_manager_sidecar && manager_sidecar_exists(&session_dir));
    if has_custom_schema {
        let Some(contents) = existing_contents.as_deref() else {
            bail!("Expected existing result content when custom schema was detected");
        };
        preserve_user_result_snapshot(&session_dir, contents)?;
    }
    retain_sidecar_result_artifacts_if_present(
        &session_dir,
        &mut persisted_result,
        preserve_existing_manager_sidecar,
    )?;
    let manager_sidecar = if let Some(sidecar) = requested_manager_sidecar {
        Some(
            manager_result_spillover::prepare_manager_sidecar_for_publish(
                &session_dir,
                &mut persisted_result,
                sidecar,
                spill_threshold_bytes,
            )?,
        )
    } else if preserve_existing_manager_sidecar {
        manager_result_spillover::load_existing_manager_sidecar_for_publish(
            &session_dir,
            &mut persisted_result,
            spill_threshold_bytes,
        )?
    } else {
        None
    };
    if manager_sidecar.is_none() && preserve_existing_manager_sidecar {
        ensure_result_artifact(&mut persisted_result, CONTRACT_RESULT_ARTIFACT_PATH);
    } else {
        remove_result_artifact(&mut persisted_result, CONTRACT_RESULT_ARTIFACT_PATH);
    }
    let clear_manager_sidecar_after_publish =
        manager_sidecar.is_none() && options.clear_stale_manager_sidecar;
    if let Some(sidecar) = manager_sidecar.as_ref() {
        // Publish the sidecar before the envelope so the authoritative
        // envelope never points at a sidecar path that failed to write.
        write_result_sidecar(&session_dir, CONTRACT_RESULT_ARTIFACT_PATH, sidecar)?;
        ensure_result_artifact(&mut persisted_result, CONTRACT_RESULT_ARTIFACT_PATH);
    }

    let runtime_table = session_result_to_table(&persisted_result)?;
    let mut merged_table = existing_table.unwrap_or_default();
    for key in RUNTIME_RESULT_KEYS {
        merged_table.remove(key);
    }
    merged_table.extend(runtime_table);
    let contents = toml::to_string_pretty(&toml::Value::Table(merged_table))
        .context("Failed to serialize session result")?;
    write_file_atomically(&result_path, &contents)
        .with_context(|| format!("Failed to write result: {}", result_path.display()))?;
    if clear_manager_sidecar_after_publish {
        // Mirror round-7's write-path atomicity: on clear, publish the
        // envelope first so it never points at a sidecar that was deleted
        // before the new envelope became durable.
        if let Err(err) = clear_manager_sidecar(&session_dir) {
            // NOTE: stale sidecar on disk is harmless. load_result_in gates the overlay
            // on envelope.artifacts containing CONTRACT_RESULT_ARTIFACT_PATH (#956 Option A),
            // so even if the unlink fails, the stale sidecar will be ignored on reload.
            tracing::warn!(
                path = %contract_result_path(&session_dir).display(),
                error = %err,
                "Failed to remove manager sidecar after envelope publication"
            );
        }
    }
    Ok(())
}

fn manager_sidecar_exists(session_dir: &Path) -> bool {
    session_dir.join(CONTRACT_RESULT_ARTIFACT_PATH).exists()
}

pub fn clear_manager_sidecar(session_dir: &Path) -> Result<()> {
    let manager_sidecar_path = session_dir.join(CONTRACT_RESULT_ARTIFACT_PATH);
    if !manager_sidecar_path.exists() {
        return Ok(());
    }
    if !manager_sidecar_path.is_file() {
        bail!(
            "Manager sidecar path exists but is not a file: {}",
            manager_sidecar_path.display()
        );
    }
    match fs::remove_file(&manager_sidecar_path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| {
            format!(
                "Failed to remove manager sidecar: {}",
                manager_sidecar_path.display()
            )
        }),
    }
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

pub fn contract_result_path(session_dir: &Path) -> PathBuf {
    session_dir.join(CONTRACT_RESULT_ARTIFACT_PATH)
}

pub fn legacy_user_result_path(session_dir: &Path) -> PathBuf {
    session_dir.join(LEGACY_USER_RESULT_ARTIFACT_PATH)
}

fn retain_sidecar_result_artifacts_if_present(
    session_dir: &Path,
    result: &mut SessionResult,
    retain_contract_sidecar: bool,
) -> Result<()> {
    if retain_contract_sidecar {
        retain_result_artifact_if_present(session_dir, result, CONTRACT_RESULT_ARTIFACT_PATH)?;
    }
    retain_result_artifact_if_present(session_dir, result, LEGACY_USER_RESULT_ARTIFACT_PATH)?;
    Ok(())
}

fn retain_result_artifact_if_present(
    session_dir: &Path,
    result: &mut SessionResult,
    artifact_path: &str,
) -> Result<()> {
    let snapshot_path = session_dir.join(artifact_path);
    if !snapshot_path.exists() {
        return Ok(());
    }
    if !snapshot_path.is_file() {
        bail!(
            "Result artifact path exists but is not a file: {}",
            snapshot_path.display()
        );
    }
    ensure_result_artifact(result, artifact_path);
    Ok(())
}

fn ensure_result_artifact(result: &mut SessionResult, artifact_path: &str) {
    if result
        .artifacts
        .iter()
        .any(|artifact| artifact.path == artifact_path)
    {
        return;
    }
    result.artifacts.push(SessionArtifact::new(artifact_path));
}

fn remove_result_artifact(result: &mut SessionResult, artifact_path: &str) {
    result
        .artifacts
        .retain(|artifact| artifact.path != artifact_path);
}

fn write_result_sidecar(
    session_dir: &Path,
    artifact_path: &str,
    sidecar: &toml::Value,
) -> Result<()> {
    let sidecar_path = session_dir.join(artifact_path);
    let Some(parent_dir) = sidecar_path.parent() else {
        bail!(
            "Result sidecar path has no parent: {}",
            sidecar_path.display()
        );
    };
    fs::create_dir_all(parent_dir)
        .with_context(|| format!("Failed to create sidecar dir: {}", parent_dir.display()))?;
    let rendered = toml::to_string_pretty(sidecar).context("Failed to serialize result sidecar")?;
    write_file_atomically(&sidecar_path, &rendered)
        .with_context(|| format!("Failed to write result sidecar: {}", sidecar_path.display()))
}

fn write_file_atomically(path: &Path, contents: &str) -> Result<()> {
    let Some(parent_dir) = path.parent() else {
        bail!("Path has no parent for atomic write: {}", path.display());
    };
    fs::create_dir_all(parent_dir)
        .with_context(|| format!("Failed to create parent dir: {}", parent_dir.display()))?;
    let mut temp_file = tempfile::NamedTempFile::new_in(parent_dir)
        .with_context(|| format!("Failed to create temp file in {}", parent_dir.display()))?;
    temp_file
        .write_all(contents.as_bytes())
        .with_context(|| format!("Failed to write temp file for {}", path.display()))?;
    temp_file
        .flush()
        .with_context(|| format!("Failed to flush temp file for {}", path.display()))?;
    temp_file
        .persist(path)
        .map_err(|err| err.error)
        .with_context(|| format!("Failed to atomically replace {}", path.display()))?;
    Ok(())
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

fn redact_toml_value(value: &mut toml::Value, key: Option<&str>) -> Result<()> {
    if key.is_some_and(is_sensitive_key) {
        *value = toml::Value::String("[REDACTED]".to_string());
        return Ok(());
    }

    match value {
        toml::Value::Table(table) => {
            for (child_key, child_value) in table {
                redact_toml_value(child_value, Some(child_key.as_str()))?;
            }
        }
        toml::Value::Array(items) => {
            for item in items {
                redact_toml_value(item, None)?;
            }
        }
        toml::Value::String(text) => {
            let serialized =
                serde_json::to_string(text).context("Failed to serialize result sidecar string")?;
            let redacted = crate::redact::redact_event(&serialized);
            *text = serde_json::from_str(&redacted)
                .context("Failed to parse redacted result sidecar string")?;
        }
        _ => {}
    }

    Ok(())
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized: String = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    matches!(
        normalized.as_str(),
        "password"
            | "passwd"
            | "pwd"
            | "secret"
            | "clientsecret"
            | "apikey"
            | "token"
            | "accesstoken"
            | "refreshtoken"
            | "idtoken"
    )
}

/// Load a session result
pub fn load_result(project_path: &Path, session_id: &str) -> Result<Option<SessionResult>> {
    let base_dir = super::resolve_read_base_dir(project_path, Some(session_id))?;
    load_result_in(&base_dir, session_id)
}

pub fn load_result_view(
    project_path: &Path,
    session_id: &str,
) -> Result<Option<SessionResultView>> {
    let base_dir = super::resolve_read_base_dir(project_path, Some(session_id))?;
    load_result_view_in(&base_dir, session_id)
}

pub(crate) fn load_result_in(base_dir: &Path, session_id: &str) -> Result<Option<SessionResult>> {
    validate_session_id(session_id)?;
    let session_dir = super::get_session_dir_in(base_dir, session_id);
    let result_path = session_dir.join(RESULT_FILE_NAME);
    let manager_sidecar_path = session_dir.join(CONTRACT_RESULT_ARTIFACT_PATH);
    if !result_path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(&result_path)
        .with_context(|| format!("Failed to read result: {}", result_path.display()))?;
    let mut result: SessionResult = toml::from_str(&contents)
        .with_context(|| format!("Failed to parse result: {}", result_path.display()))?;
    let envelope_references_manager_sidecar = result
        .artifacts
        .iter()
        .any(|artifact| artifact.path == CONTRACT_RESULT_ARTIFACT_PATH);
    if envelope_references_manager_sidecar {
        let manager_sidecar = load_optional_result_sidecar(
            &session_dir,
            CONTRACT_RESULT_ARTIFACT_PATH,
        )
        .unwrap_or_else(|error| {
            tracing::warn!(
                target: "csa-session.load_result",
                path = %manager_sidecar_path.display(),
                error = %error,
                "sidecar present but unreadable/malformed; ignoring (runtime envelope still loaded)"
            );
            None
        });
        if let Some(sidecar) = manager_sidecar {
            result.manager_fields = crate::result::SessionManagerFields::from_sidecar(&sidecar);
        }
    }
    Ok(Some(result))
}

pub(crate) fn load_result_view_in(
    base_dir: &Path,
    session_id: &str,
) -> Result<Option<SessionResultView>> {
    let Some(envelope) = load_result_in(base_dir, session_id)? else {
        return Ok(None);
    };
    let session_dir = super::get_session_dir_in(base_dir, session_id);
    let manager_sidecar = if envelope
        .artifacts
        .iter()
        .any(|artifact| artifact.path == CONTRACT_RESULT_ARTIFACT_PATH)
    {
        load_optional_result_sidecar(&session_dir, CONTRACT_RESULT_ARTIFACT_PATH)?
    } else {
        None
    };
    Ok(Some(SessionResultView {
        envelope,
        manager_sidecar,
        legacy_sidecar: load_optional_result_sidecar(
            &session_dir,
            LEGACY_USER_RESULT_ARTIFACT_PATH,
        )?,
    }))
}

fn load_optional_result_sidecar(
    session_dir: &Path,
    artifact_path: &str,
) -> Result<Option<toml::Value>> {
    let sidecar_path = session_dir.join(artifact_path);
    if !sidecar_path.exists() {
        return Ok(None);
    }
    if !sidecar_path.is_file() {
        bail!(
            "Result artifact path exists but is not a file: {}",
            sidecar_path.display()
        );
    }
    let contents = fs::read_to_string(&sidecar_path)
        .with_context(|| format!("Failed to read result artifact: {}", sidecar_path.display()))?;
    let sidecar = toml::from_str(&contents).with_context(|| {
        format!(
            "Failed to parse result artifact: {}",
            sidecar_path.display()
        )
    })?;
    Ok(Some(sidecar))
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
