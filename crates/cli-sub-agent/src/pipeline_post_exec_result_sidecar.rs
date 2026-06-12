use std::{fs, path::Path};

pub(super) fn ensure_turn_scoped_manager_artifact(
    session_dir: &Path,
    completed_turn_count: u32,
    result: &mut csa_session::SessionResult,
) {
    if let Some(artifact_path) = current_result_artifact_marker(session_dir)
        && (result.manager_fields.as_sidecar().is_some()
            || session_dir.join(&artifact_path).is_file())
    {
        ensure_owned_manager_result_artifact(result, artifact_path);
        return;
    }

    if result.manager_fields.as_sidecar().is_none()
        || result.artifacts.iter().any(|artifact| {
            !artifact.display_only && csa_session::is_manager_result_artifact_path(&artifact.path)
        })
    {
        return;
    }

    ensure_owned_manager_result_artifact(
        result,
        csa_session::turn_contract_result_artifact_path(completed_turn_count),
    );
}

fn current_result_artifact_marker(session_dir: &Path) -> Option<String> {
    let marker_path =
        crate::pipeline::result_contract::current_result_artifact_marker_path(session_dir);
    let contents = fs::read_to_string(marker_path).ok()?;
    let marker: toml::Value = toml::from_str(&contents).ok()?;
    let artifact_path = marker.get("artifact_path")?.as_str()?.to_string();
    csa_session::is_manager_result_artifact_path(&artifact_path).then_some(artifact_path)
}

fn ensure_owned_manager_result_artifact(
    result: &mut csa_session::SessionResult,
    artifact_path: String,
) {
    result.artifacts.retain(|artifact| {
        artifact.path != artifact_path
            && (artifact.display_only
                || !csa_session::is_manager_result_artifact_path(&artifact.path))
    });
    result
        .artifacts
        .push(csa_session::SessionArtifact::new(artifact_path));
    result
        .artifacts
        .sort_by(|left, right| left.path.cmp(&right.path));
}

pub(super) fn status_is_success(session_dir: &Path, completed_turn_count: u32) -> bool {
    let turn_scoped_path =
        csa_session::turn_contract_result_path(session_dir, completed_turn_count);
    if path_status_is_success(&turn_scoped_path) {
        return true;
    }

    path_status_is_success(&csa_session::contract_result_path(session_dir))
}

fn path_status_is_success(path: &Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(toml::Value::Table(table)) = toml::from_str::<toml::Value>(&contents) else {
        return false;
    };

    let nested = table
        .get("result")
        .and_then(|value| value.as_table())
        .and_then(|table| table.get("status"));
    let flat = table.get("status");

    nested
        .or(flat)
        .and_then(|value| value.as_str())
        .is_some_and(|status| status.eq_ignore_ascii_case("success"))
}

#[cfg(test)]
#[path = "pipeline_post_exec_result_sidecar_tests.rs"]
mod tests;
