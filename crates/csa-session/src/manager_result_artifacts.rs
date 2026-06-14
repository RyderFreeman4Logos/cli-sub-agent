use crate::result::{RESULT_FILE_NAME, SessionArtifact, SessionResult};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use super::{CONTRACT_RESULT_ARTIFACT_PATH, LEGACY_USER_RESULT_ARTIFACT_PATH};

const TURN_CONTRACT_RESULT_PREFIX: &str = "output/turns/turn-";
const TURN_CONTRACT_RESULT_SUFFIX: &str = "/result.toml";

pub fn contract_result_path(session_dir: &Path) -> PathBuf {
    session_dir.join(CONTRACT_RESULT_ARTIFACT_PATH)
}

pub fn legacy_user_result_path(session_dir: &Path) -> PathBuf {
    session_dir.join(LEGACY_USER_RESULT_ARTIFACT_PATH)
}

pub fn turn_contract_result_artifact_path(turn_number: u32) -> String {
    let turn_number = turn_number.max(1);
    format!("output/turns/turn-{turn_number:06}/result.toml")
}

pub fn turn_contract_result_path(session_dir: &Path, turn_number: u32) -> PathBuf {
    session_dir.join(turn_contract_result_artifact_path(turn_number))
}

pub fn next_turn_contract_result_artifact_path(completed_turn_count: u32) -> String {
    turn_contract_result_artifact_path(completed_turn_count.saturating_add(1))
}

pub fn next_turn_contract_result_path(session_dir: &Path, completed_turn_count: u32) -> PathBuf {
    session_dir.join(next_turn_contract_result_artifact_path(
        completed_turn_count,
    ))
}

pub fn existing_turn_contract_result_artifact_path(
    session_dir: &Path,
    turn_number: u32,
) -> Option<String> {
    let artifact_path = turn_contract_result_artifact_path(turn_number);
    session_dir
        .join(&artifact_path)
        .is_file()
        .then_some(artifact_path)
}

pub fn existing_next_turn_contract_result_artifact_path(
    session_dir: &Path,
    completed_turn_count: u32,
) -> Option<String> {
    existing_turn_contract_result_artifact_path(session_dir, completed_turn_count.saturating_add(1))
}

pub fn is_manager_result_artifact_path(artifact_path: &str) -> bool {
    artifact_path == CONTRACT_RESULT_ARTIFACT_PATH
        || parse_turn_contract_result_artifact_path(artifact_path).is_some()
}

/// Convert an observed session artifact path into an ownership-safe artifact.
///
/// Manager result artifacts and gate-failure logs discovered by directory
/// scans are diagnostics only: callers must prove current-turn ownership
/// before such paths can drive result repair or manager sidecar selection.
pub fn observed_session_artifact(artifact_path: impl Into<String>) -> SessionArtifact {
    let artifact_path = artifact_path.into();
    if is_manager_result_artifact_path(&artifact_path)
        || artifact_path == crate::post_exec_gate_report::GATE_FAILURE_LOG_REL_PATH
    {
        SessionArtifact::display_only(artifact_path)
    } else {
        SessionArtifact::new(artifact_path)
    }
}

pub fn latest_manager_result_artifact_path(session_dir: &Path) -> Option<String> {
    latest_turn_contract_result_artifact_path(session_dir).or_else(|| {
        session_dir
            .join(CONTRACT_RESULT_ARTIFACT_PATH)
            .is_file()
            .then(|| CONTRACT_RESULT_ARTIFACT_PATH.to_string())
    })
}

pub(super) fn remove_manager_result_artifacts(result: &mut SessionResult) {
    result.artifacts.retain(|artifact| {
        artifact.display_only || !is_manager_result_artifact_path(&artifact.path)
    });
}

pub(super) fn select_manager_sidecar_artifact_path(
    result: &SessionResult,
    has_requested_manager_sidecar: bool,
) -> String {
    result
        .artifacts
        .iter()
        .filter(|artifact| !artifact.display_only)
        .filter_map(|artifact| parse_turn_contract_result_artifact_path(&artifact.path))
        .max()
        .map(turn_contract_result_artifact_path)
        .or_else(|| {
            result
                .artifacts
                .iter()
                .filter(|artifact| !artifact.display_only)
                .any(|artifact| artifact.path == CONTRACT_RESULT_ARTIFACT_PATH)
                .then(|| CONTRACT_RESULT_ARTIFACT_PATH.to_string())
        })
        .or_else(|| {
            if has_requested_manager_sidecar {
                Some(CONTRACT_RESULT_ARTIFACT_PATH.to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| CONTRACT_RESULT_ARTIFACT_PATH.to_string())
}

pub(super) fn referenced_manager_sidecar_artifact(result: &SessionResult) -> Option<&str> {
    result
        .artifacts
        .iter()
        .filter(|artifact| !artifact.display_only)
        .filter_map(|artifact| {
            parse_turn_contract_result_artifact_path(&artifact.path)
                .map(|turn| (turn, artifact.path.as_str()))
        })
        .max_by_key(|(turn, _)| *turn)
        .map(|(_, path)| path)
        .or_else(|| {
            result
                .artifacts
                .iter()
                .filter(|artifact| !artifact.display_only)
                .any(|artifact| artifact.path == CONTRACT_RESULT_ARTIFACT_PATH)
                .then_some(CONTRACT_RESULT_ARTIFACT_PATH)
        })
}

pub(super) fn collect_output_artifacts(
    output_dir: &Path,
    current_dir: &Path,
    artifacts: &mut Vec<String>,
) -> Result<()> {
    for entry in fs::read_dir(current_dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_dir() {
            collect_output_artifacts(output_dir, &path, artifacts)?;
        } else if file_type.is_file() {
            let relative = path.strip_prefix(output_dir).with_context(|| {
                format!(
                    "Failed to relativize artifact {} against {}",
                    path.display(),
                    output_dir.display()
                )
            })?;
            artifacts.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

fn latest_turn_contract_result_artifact_path(session_dir: &Path) -> Option<String> {
    let entries = fs::read_dir(session_dir.join("output").join("turns")).ok()?;
    entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            if !entry.file_type().ok()?.is_dir() {
                return None;
            }
            let name = entry.file_name();
            let name = name.to_str()?;
            let turn_number = name.strip_prefix("turn-")?.parse::<u32>().ok()?;
            entry
                .path()
                .join(RESULT_FILE_NAME)
                .is_file()
                .then_some(turn_number)
        })
        .max()
        .map(turn_contract_result_artifact_path)
}

fn parse_turn_contract_result_artifact_path(artifact_path: &str) -> Option<u32> {
    let turn = artifact_path
        .strip_prefix(TURN_CONTRACT_RESULT_PREFIX)?
        .strip_suffix(TURN_CONTRACT_RESULT_SUFFIX)?;
    turn.parse::<u32>().ok().filter(|turn| *turn > 0)
}
