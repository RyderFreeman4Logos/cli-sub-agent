use std::collections::HashMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use tracing::info;

pub(crate) const CARGO_TARGET_POLICY_ARTIFACT: &str = "output/cargo-target-policy.toml";
const CARGO_TARGET_PROBE_PREFIX: &str = ".csa-cargo-target-probe";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct CargoTargetPolicyReport {
    pub(crate) schema_version: u8,
    pub(crate) original_workspace_target: String,
    pub(crate) selected_cargo_target: String,
    pub(crate) policy_reason: String,
    pub(crate) workspace_target_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) workspace_target_error: Option<String>,
    pub(crate) explicit_override_preserved: bool,
    pub(crate) automatic_substitution_applied: bool,
}

impl CargoTargetPolicyReport {
    fn new(
        original_workspace_target: &Path,
        selected_cargo_target: &Path,
        policy_reason: impl Into<String>,
        workspace_target_status: impl Into<String>,
        workspace_target_error: Option<String>,
        explicit_override_preserved: bool,
        automatic_substitution_applied: bool,
    ) -> Self {
        Self {
            schema_version: 1,
            original_workspace_target: original_workspace_target.to_string_lossy().into_owned(),
            selected_cargo_target: selected_cargo_target.to_string_lossy().into_owned(),
            policy_reason: policy_reason.into(),
            workspace_target_status: workspace_target_status.into(),
            workspace_target_error,
            explicit_override_preserved,
            automatic_substitution_applied,
        }
    }

    pub(crate) fn should_persist_artifact(&self) -> bool {
        self.explicit_override_preserved || self.automatic_substitution_applied
    }
}

pub(crate) fn apply_review_target_dir(project_root: &Path, tool_name: &str) {
    let repo_target_dir = project_root.join("target");
    if let Some(target_kind) = detect_project_target_kind(&repo_target_dir) {
        info!(
            project_target = %repo_target_dir.display(),
            tool = tool_name,
            target_kind,
            "honoring user ./target ({target_kind}), CARGO_TARGET_DIR untouched"
        );
        return;
    }

    info!(
        project_target = %repo_target_dir.display(),
        tool = tool_name,
        "no ./target present, CARGO_TARGET_DIR left at codex/cargo default"
    );
}

#[cfg(test)]
pub(crate) fn apply_task_target_dir_guards(
    task_type: Option<&str>,
    tool_name: &str,
    project_root: &Path,
    merged_env: &mut HashMap<String, String>,
) -> Result<CargoTargetPolicyReport, String> {
    if matches!(task_type, Some("review")) {
        apply_review_target_dir(project_root, tool_name);
    }
    apply_run_target_dir_guard_inner(task_type, tool_name, project_root, merged_env, true)
}

pub(crate) fn apply_runtime_task_target_dir_guards(
    task_type: Option<&str>,
    tool_name: &str,
    project_root: &Path,
    merged_env: &mut HashMap<String, String>,
    caller_env: Option<&HashMap<String, String>>,
) -> Result<CargoTargetPolicyReport, String> {
    if matches!(task_type, Some("review")) {
        apply_review_target_dir(project_root, tool_name);
    }
    apply_run_target_dir_guard_inner(
        task_type,
        tool_name,
        project_root,
        merged_env,
        caller_preserved_cargo_target_override(caller_env, merged_env),
    )
}

#[cfg(test)]
pub(crate) fn apply_run_target_dir_guard(
    task_type: Option<&str>,
    tool_name: &str,
    project_root: &Path,
    merged_env: &mut HashMap<String, String>,
) -> Result<CargoTargetPolicyReport, String> {
    apply_run_target_dir_guard_inner(task_type, tool_name, project_root, merged_env, true)
}

fn apply_run_target_dir_guard_inner(
    task_type: Option<&str>,
    tool_name: &str,
    project_root: &Path,
    merged_env: &mut HashMap<String, String>,
    preserve_existing_target_env: bool,
) -> Result<CargoTargetPolicyReport, String> {
    let workspace_target = project_root.join("target");
    if !matches!(task_type, Some("run")) {
        return Ok(CargoTargetPolicyReport::new(
            &workspace_target,
            &workspace_target,
            "not_applicable",
            "not_checked",
            None,
            false,
            false,
        ));
    }

    if preserve_existing_target_env
        && let Some(explicit_target) = explicit_cargo_target_override(merged_env)
    {
        info!(
            project_target = %workspace_target.display(),
            selected_cargo_target = %explicit_target.display(),
            tool = tool_name,
            "Run session: explicit CARGO_TARGET_DIR preserved"
        );
        return Ok(CargoTargetPolicyReport::new(
            &workspace_target,
            &explicit_target,
            "explicit_override_preserved",
            "not_checked",
            None,
            true,
            false,
        ));
    }

    match workspace_target_writeability(&workspace_target) {
        WorkspaceTargetWriteability::Writable { status } => {
            info!(
                project_target = %workspace_target.display(),
                workspace_target_status = status,
                tool = tool_name,
                "Run session: workspace Cargo target is writable; CARGO_TARGET_DIR untouched"
            );
            Ok(CargoTargetPolicyReport::new(
                &workspace_target,
                &workspace_target,
                "workspace_target_writable",
                status,
                None,
                false,
                false,
            ))
        }
        WorkspaceTargetWriteability::Unavailable { status, error }
        | WorkspaceTargetWriteability::Unwritable { status, error } => {
            let managed_target = prepare_managed_cargo_target_dir(project_root)?;
            merged_env.insert(
                csa_core::env::CARGO_TARGET_DIR_ENV_KEY.to_string(),
                managed_target.to_string_lossy().into_owned(),
            );
            info!(
                project_target = %workspace_target.display(),
                selected_cargo_target = %managed_target.display(),
                workspace_target_status = status,
                workspace_target_error = error.as_deref().unwrap_or(""),
                tool = tool_name,
                "Run session: selected CSA-managed Cargo target because workspace target is not writable"
            );
            Ok(CargoTargetPolicyReport::new(
                &workspace_target,
                &managed_target,
                "managed_target_selected",
                status,
                error,
                false,
                true,
            ))
        }
    }
}

pub(crate) fn persist_cargo_target_policy_artifact(
    session_dir: &Path,
    report: &CargoTargetPolicyReport,
) -> std::io::Result<()> {
    let artifact_path = session_dir.join(CARGO_TARGET_POLICY_ARTIFACT);
    if let Some(parent) = artifact_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = toml::to_string_pretty(report).map_err(std::io::Error::other)?;
    std::fs::write(artifact_path, body)
}

fn explicit_cargo_target_override(merged_env: &HashMap<String, String>) -> Option<PathBuf> {
    let value = merged_env
        .get(csa_core::env::CARGO_TARGET_DIR_ENV_KEY)
        .filter(|value| !value.trim().is_empty())?;
    Some(PathBuf::from(value))
}

fn caller_preserved_cargo_target_override(
    caller_env: Option<&HashMap<String, String>>,
    merged_env: &HashMap<String, String>,
) -> bool {
    if let Some(caller_value) = caller_env
        .and_then(|env| env.get(csa_core::env::CARGO_TARGET_DIR_ENV_KEY))
        .filter(|value| !value.trim().is_empty())
    {
        return cargo_target_override_was_preserved(caller_value, merged_env);
    }

    std::env::var_os(csa_core::env::CARGO_TARGET_DIR_ENV_KEY)
        .filter(|value| !value.is_empty())
        .is_some_and(|caller_value| {
            if csa_core::env::rust_state_path_needs_session_override(Path::new(&caller_value)) {
                return false;
            }
            let caller_path = PathBuf::from(caller_value);
            merged_env
                .get(csa_core::env::CARGO_TARGET_DIR_ENV_KEY)
                .filter(|value| !value.trim().is_empty())
                .is_some_and(|merged_value| Path::new(merged_value) == caller_path.as_path())
        })
}

fn cargo_target_override_was_preserved(
    caller_value: &str,
    merged_env: &HashMap<String, String>,
) -> bool {
    if csa_core::env::rust_state_path_needs_session_override(Path::new(caller_value)) {
        return false;
    }

    merged_env
        .get(csa_core::env::CARGO_TARGET_DIR_ENV_KEY)
        .filter(|value| !value.trim().is_empty())
        .is_some_and(|merged_value| Path::new(merged_value) == Path::new(caller_value))
}

enum WorkspaceTargetWriteability {
    Writable {
        status: &'static str,
    },
    Unavailable {
        status: &'static str,
        error: Option<String>,
    },
    Unwritable {
        status: &'static str,
        error: Option<String>,
    },
}

fn workspace_target_writeability(workspace_target: &Path) -> WorkspaceTargetWriteability {
    let metadata = match std::fs::symlink_metadata(workspace_target) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return WorkspaceTargetWriteability::Writable {
                status: "workspace_target_absent_cargo_default",
            };
        }
        Err(error) => {
            return WorkspaceTargetWriteability::Unavailable {
                status: "workspace_target_metadata_error",
                error: Some(error.to_string()),
            };
        }
    };

    if metadata.file_type().is_symlink() {
        match std::fs::metadata(workspace_target) {
            Ok(target_metadata) if target_metadata.is_dir() => {}
            Ok(_) => {
                return WorkspaceTargetWriteability::Unavailable {
                    status: "workspace_target_symlink_not_directory",
                    error: None,
                };
            }
            Err(error) => {
                return WorkspaceTargetWriteability::Unavailable {
                    status: "workspace_target_symlink_unavailable",
                    error: Some(error.to_string()),
                };
            }
        }
    } else if !metadata.is_dir() {
        return WorkspaceTargetWriteability::Unavailable {
            status: "workspace_target_not_directory",
            error: None,
        };
    }

    match writable_directory_probe(workspace_target) {
        Ok(()) => WorkspaceTargetWriteability::Writable {
            status: "workspace_target_writable",
        },
        Err(error) => WorkspaceTargetWriteability::Unwritable {
            status: "workspace_target_unwritable",
            error: Some(error),
        },
    }
}

fn prepare_managed_cargo_target_dir(project_root: &Path) -> Result<PathBuf, String> {
    let session_root = csa_session::manager::get_session_root(project_root).map_err(|error| {
        format!(
            "Failed to resolve CSA-managed Cargo target directory for project '{}': {error}",
            project_root.display()
        )
    })?;
    let managed_target = session_root.join("cargo-target");
    std::fs::create_dir_all(&managed_target).map_err(|error| {
        format!(
            "Failed to create CSA-managed Cargo target directory '{}': {error}. \
             CSA will not try fallback Cargo target directories; set CARGO_TARGET_DIR explicitly \
             or fix state directory permissions.",
            managed_target.display()
        )
    })?;
    writable_directory_probe(&managed_target).map_err(|error| {
        format!(
            "CSA-managed Cargo target directory '{}' is not writable: {error}. \
             CSA will not try fallback Cargo target directories; set CARGO_TARGET_DIR explicitly \
             or fix state directory permissions.",
            managed_target.display()
        )
    })?;
    Ok(managed_target)
}

fn writable_directory_probe(dir: &Path) -> Result<(), String> {
    for attempt in 0..8 {
        let probe = dir.join(format!(
            "{CARGO_TARGET_PROBE_PREFIX}-{}-{attempt}",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&probe) {
            Ok(_) => {
                let _ = std::fs::remove_file(&probe);
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }

    Err("probe path collision after 8 attempts".to_string())
}

fn detect_project_target_kind(repo_target_dir: &Path) -> Option<&'static str> {
    let metadata = std::fs::symlink_metadata(repo_target_dir).ok()?;
    if metadata.file_type().is_symlink() {
        return Some("symlink");
    }
    if metadata.is_dir() {
        return Some("dir");
    }
    None
}
