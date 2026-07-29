//! Attempt-scoped receipt preparation for the result.toml contract.

use std::path::Path;

use tracing::warn;

use super::{
    CURRENT_RESULT_ARTIFACT_FILE, clear_expected_result_toml, ensure_expected_result_parent_dir,
    prompt_requires_result_toml_path,
};

pub(crate) fn clear_expected_result_artifacts_for_prompt(
    prompt: &str,
    session_dir: &Path,
    completed_turn_count: u32,
) -> bool {
    let session_result_path = session_dir.join("result.toml");
    let session_cleared = clear_expected_result_toml(&session_result_path);
    if !prompt_requires_result_toml_path(prompt) {
        clear_current_result_artifact_marker(session_dir);
        return session_cleared;
    }

    let turn_output_path =
        csa_session::next_turn_contract_result_path(session_dir, completed_turn_count);
    let turn_output_parent_ready = ensure_expected_result_parent_dir(&turn_output_path);
    let contract_output_path = csa_session::contract_result_path(session_dir);
    let legacy_output_path = csa_session::legacy_user_result_path(session_dir);
    let turn_output_cleared = clear_expected_result_toml(&turn_output_path);
    let contract_output_cleared = clear_expected_result_toml(&contract_output_path);
    let legacy_output_cleared = clear_expected_result_toml(&legacy_output_path);
    let prepared = session_cleared
        && turn_output_parent_ready
        && turn_output_cleared
        && contract_output_cleared
        && legacy_output_cleared;
    if prepared {
        let attempt_nonce = ulid::Ulid::new().to_string();
        if persist_current_result_artifact_marker(session_dir, completed_turn_count, &attempt_nonce)
        {
            return true;
        }
    }
    clear_current_result_artifact_marker(session_dir);
    false
}

pub(crate) fn current_result_attempt_nonce(session_dir: &Path) -> Option<String> {
    let marker_path = current_result_artifact_marker_path(session_dir);
    let contents = std::fs::read_to_string(marker_path).ok()?;
    let marker: toml::Value = toml::from_str(&contents).ok()?;
    marker.get("attempt_nonce")?.as_str().map(str::to_owned)
}

pub(crate) fn current_result_artifact_marker_path(session_dir: &Path) -> std::path::PathBuf {
    session_dir.join(CURRENT_RESULT_ARTIFACT_FILE)
}

fn persist_current_result_artifact_marker(
    session_dir: &Path,
    completed_turn_count: u32,
    attempt_nonce: &str,
) -> bool {
    let artifact_path = csa_session::next_turn_contract_result_artifact_path(completed_turn_count);
    let mut table = toml::Table::new();
    table.insert(
        "artifact_path".to_string(),
        toml::Value::String(artifact_path),
    );
    table.insert(
        "attempt_nonce".to_string(),
        toml::Value::String(attempt_nonce.to_string()),
    );
    let marker_path = current_result_artifact_marker_path(session_dir);
    let contents = match toml::to_string_pretty(&table) {
        Ok(contents) => contents,
        Err(err) => {
            warn!(
                path = %marker_path.display(),
                error = %err,
                "Failed to serialize current result artifact marker"
            );
            return false;
        }
    };
    match std::fs::write(&marker_path, contents) {
        Ok(()) => true,
        Err(err) => {
            warn!(
                path = %marker_path.display(),
                error = %err,
                "Failed to persist current result artifact marker"
            );
            false
        }
    }
}

fn clear_current_result_artifact_marker(session_dir: &Path) {
    let marker_path = current_result_artifact_marker_path(session_dir);
    match std::fs::remove_file(&marker_path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => warn!(
            path = %marker_path.display(),
            error = %err,
            "Failed to remove stale current result artifact marker"
        ),
    }
}
