use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::InheritedModelPin;
use crate::startup_env::StartupSubtreeEnv;

const SUBTREE_MODEL_PIN_SIDECAR: &str = "subtree-model-pin.toml";
const SUBTREE_MODEL_PIN_SIDECAR_SCHEMA_VERSION: u8 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SubtreeModelPinSidecar {
    schema_version: u8,
    session_id: String,
    project_root: String,
    session_dir: String,
    model_spec: String,
    force_ignore_tier_setting: bool,
    no_failover: bool,
}

impl SubtreeModelPinSidecar {
    fn from_pin(
        project_root: &Path,
        session_id: &str,
        session_dir: &Path,
        pin: &csa_core::env::SubtreeModelPin,
    ) -> Self {
        Self {
            schema_version: SUBTREE_MODEL_PIN_SIDECAR_SCHEMA_VERSION,
            session_id: session_id.to_string(),
            project_root: project_root.to_string_lossy().into_owned(),
            session_dir: session_dir.to_string_lossy().into_owned(),
            model_spec: pin.model_spec().to_string(),
            force_ignore_tier_setting: true,
            no_failover: pin.no_failover(),
        }
    }

    fn matches_inherited_pin(
        &self,
        pin: &InheritedModelPin,
        project_root: &Path,
        session_id: &str,
        session_dir: &Path,
    ) -> bool {
        self.schema_version == SUBTREE_MODEL_PIN_SIDECAR_SCHEMA_VERSION
            && self.session_id == session_id
            && paths_equivalent(Path::new(&self.project_root), project_root)
            && paths_equivalent(Path::new(&self.session_dir), session_dir)
            && self.model_spec == pin.model_spec
            && self.force_ignore_tier_setting == pin.force_ignore_tier_setting
            && self.no_failover == pin.no_failover
    }
}

pub(crate) fn sync_subtree_model_pin_sidecar(
    project_root: &Path,
    session_id: &str,
    session_dir: &Path,
    pin: Option<&csa_core::env::SubtreeModelPin>,
) -> Result<()> {
    let path = subtree_model_pin_sidecar_path(session_dir);
    let Some(pin) = pin else {
        match std::fs::remove_file(&path) {
            Ok(()) => return Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => {
                return Err(err).with_context(|| {
                    format!(
                        "Failed to remove stale subtree model pin sidecar: {}",
                        path.display()
                    )
                });
            }
        }
    };

    std::fs::create_dir_all(session_dir).with_context(|| {
        format!(
            "Failed to create session directory for subtree model pin sidecar: {}",
            session_dir.display()
        )
    })?;
    let contents = toml::to_string_pretty(&SubtreeModelPinSidecar::from_pin(
        project_root,
        session_id,
        session_dir,
        pin,
    ))
    .context("Failed to serialize subtree model pin sidecar")?;
    std::fs::write(&path, contents).with_context(|| {
        format!(
            "Failed to write subtree model pin sidecar: {}",
            path.display()
        )
    })
}

fn subtree_model_pin_sidecar_path(session_dir: &Path) -> PathBuf {
    session_dir.join(SUBTREE_MODEL_PIN_SIDECAR)
}

pub(super) fn startup_env_sidecar_trusts_pin(
    startup_env: &StartupSubtreeEnv,
    pin: &InheritedModelPin,
) -> bool {
    let Some(session_id) = startup_env.session_id() else {
        return false;
    };
    let Some(session_dir) = startup_env.session_dir() else {
        return false;
    };
    let Some(project_root) = startup_env.project_root() else {
        return false;
    };

    let session_dir_path = Path::new(session_dir);
    let sidecar_path = subtree_model_pin_sidecar_path(session_dir_path);
    let sidecar = match read_subtree_model_pin_sidecar(&sidecar_path) {
        Ok(sidecar) => sidecar,
        Err(err) => {
            tracing::warn!(
                session_id,
                session_dir,
                sidecar_path = %sidecar_path.display(),
                error = %err,
                "ignoring inherited CSA_MODEL_SPEC because the trusted pin sidecar is missing or invalid"
            );
            return false;
        }
    };

    let project_root_path = Path::new(project_root);
    if !sidecar.matches_inherited_pin(pin, project_root_path, session_id, session_dir_path) {
        tracing::warn!(
            session_id,
            session_dir,
            sidecar_path = %sidecar_path.display(),
            env_model_spec = %pin.model_spec,
            sidecar_session_id = %sidecar.session_id,
            sidecar_project_root = %sidecar.project_root,
            sidecar_session_dir = %sidecar.session_dir,
            sidecar_model_spec = %sidecar.model_spec,
            "ignoring inherited CSA_MODEL_SPEC because the trusted pin sidecar does not match the startup env"
        );
        return false;
    }

    startup_env_session_contract_matches_state(startup_env, project_root, session_id, session_dir)
}

fn read_subtree_model_pin_sidecar(path: &Path) -> Result<SubtreeModelPinSidecar> {
    let contents = std::fs::read_to_string(path).with_context(|| {
        format!(
            "Failed to read subtree model pin sidecar: {}",
            path.display()
        )
    })?;
    toml::from_str(&contents).with_context(|| {
        format!(
            "Failed to parse subtree model pin sidecar: {}",
            path.display()
        )
    })
}

fn startup_env_session_contract_matches_state(
    startup_env: &StartupSubtreeEnv,
    project_root: &str,
    session_id: &str,
    session_dir: &str,
) -> bool {
    let project_root_path = Path::new(project_root);
    let state = match csa_session::load_session(project_root_path, session_id) {
        Ok(state) => state,
        Err(err) => {
            tracing::warn!(
                session_id,
                project_root,
                error = %err,
                "ignoring inherited CSA_MODEL_SPEC because the startup session state could not be loaded"
            );
            return false;
        }
    };

    if state.meta_session_id != session_id {
        tracing::warn!(
            session_id,
            state_session_id = %state.meta_session_id,
            "ignoring inherited CSA_MODEL_SPEC because startup session id does not match persisted state"
        );
        return false;
    }

    if !paths_equivalent(Path::new(&state.project_path), project_root_path) {
        tracing::warn!(
            session_id,
            project_root,
            state_project_root = %state.project_path,
            "ignoring inherited CSA_MODEL_SPEC because startup project root does not match persisted state"
        );
        return false;
    }

    let expected_session_dir = match csa_session::get_session_dir(project_root_path, session_id) {
        Ok(path) => path,
        Err(err) => {
            tracing::warn!(
                session_id,
                project_root,
                error = %err,
                "ignoring inherited CSA_MODEL_SPEC because the startup session directory could not be resolved"
            );
            return false;
        }
    };
    if !paths_equivalent(Path::new(session_dir), &expected_session_dir) {
        tracing::warn!(
            session_id,
            session_dir,
            expected_session_dir = %expected_session_dir.display(),
            "ignoring inherited CSA_MODEL_SPEC because startup session dir does not match persisted state"
        );
        return false;
    }

    let expected_child_depth = state.genealogy.depth.saturating_add(1);
    if startup_env.current_depth() != expected_child_depth {
        tracing::warn!(
            session_id,
            startup_depth = startup_env.current_depth(),
            state_depth = state.genealogy.depth,
            "ignoring inherited CSA_MODEL_SPEC because startup depth does not match persisted session genealogy"
        );
        return false;
    }

    if state.genealogy.parent_session_id.as_deref() != startup_env.parent_session() {
        tracing::warn!(
            session_id,
            startup_parent = startup_env.parent_session(),
            state_parent = state.genealogy.parent_session_id.as_deref(),
            "ignoring inherited CSA_MODEL_SPEC because startup parent session does not match persisted state"
        );
        return false;
    }

    if let Some(parent_session_dir) = startup_env.parent_session_dir()
        && let Some(parent_session_id) = state.genealogy.parent_session_id.as_deref()
        && !parent_session_dir_matches(project_root_path, parent_session_id, parent_session_dir)
    {
        tracing::warn!(
            session_id,
            parent_session_id,
            parent_session_dir,
            "ignoring inherited CSA_MODEL_SPEC because startup parent session dir does not match persisted state"
        );
        return false;
    }

    true
}

fn parent_session_dir_matches(
    project_root: &Path,
    parent_session_id: &str,
    parent_session_dir: &str,
) -> bool {
    csa_session::get_session_dir(project_root, parent_session_id)
        .map(|expected| paths_equivalent(Path::new(parent_session_dir), &expected))
        .unwrap_or(false)
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}
