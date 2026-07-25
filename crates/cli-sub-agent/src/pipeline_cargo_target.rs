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
    let preserve_existing_target_env = matches!(task_type, Some("run"))
        && restore_caller_cargo_target_override(caller_env, merged_env);
    apply_run_target_dir_guard_inner(
        task_type,
        tool_name,
        project_root,
        merged_env,
        preserve_existing_target_env,
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
        | WorkspaceTargetWriteability::Unwritable { status, error } => Err(
            cargo_target_preflight_error(&workspace_target, status, error.as_deref()),
        ),
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

fn restore_caller_cargo_target_override(
    caller_env: Option<&HashMap<String, String>>,
    merged_env: &mut HashMap<String, String>,
) -> bool {
    let caller_target = if let Some(caller_value) = caller_env
        .and_then(|env| env.get(csa_core::env::CARGO_TARGET_DIR_ENV_KEY))
        .filter(|value| !value.trim().is_empty())
    {
        PathBuf::from(caller_value.as_str())
    } else if let Some(caller_value) =
        std::env::var_os(csa_core::env::CARGO_TARGET_DIR_ENV_KEY).filter(|value| !value.is_empty())
    {
        PathBuf::from(caller_value)
    } else {
        return false;
    };
    if csa_core::env::rust_state_path_needs_session_override(&caller_target) {
        return false;
    }

    merged_env.insert(
        csa_core::env::CARGO_TARGET_DIR_ENV_KEY.to_string(),
        caller_target.to_string_lossy().into_owned(),
    );
    true
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

fn cargo_target_preflight_error(
    workspace_target: &Path,
    status: &str,
    error: Option<&str>,
) -> String {
    let resolved_target = resolved_target_path_for_diagnostics(workspace_target);
    let detail = error.map(|error| format!(": {error}")).unwrap_or_default();
    format!(
        "Cargo target preflight blocked before provider execution: configured target lexical path \
         '{}' resolves to '{}'; status '{status}'{detail}. CSA will not substitute an alternate \
         CARGO_TARGET_DIR. Restore the configured target directory or symlink destination and make \
         it writable; when filesystem sandboxing is enabled, grant the resolved path write access \
         through filesystem_sandbox.extra_writable before retrying.",
        workspace_target.display(),
        resolved_target.display(),
    )
}

fn resolved_target_path_for_diagnostics(workspace_target: &Path) -> PathBuf {
    let candidate = match std::fs::symlink_metadata(workspace_target) {
        Ok(metadata) if metadata.file_type().is_symlink() => std::fs::read_link(workspace_target)
            .map(|target| {
                if target.is_absolute() {
                    target
                } else {
                    workspace_target
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .join(target)
                }
            })
            .unwrap_or_else(|_| workspace_target.to_path_buf()),
        _ => workspace_target.to_path_buf(),
    };
    canonicalize_existing_prefix(&candidate)
}

fn canonicalize_existing_prefix(path: &Path) -> PathBuf {
    let mut candidate = path.to_path_buf();
    let mut missing_components = Vec::new();
    loop {
        if let Ok(mut resolved) = candidate.canonicalize() {
            for component in missing_components.iter().rev() {
                resolved.push(component);
            }
            return resolved;
        }

        let Some(component) = candidate.file_name().map(std::ffi::OsStr::to_os_string) else {
            return path.to_path_buf();
        };
        missing_components.push(component);
        if !candidate.pop() {
            return path.to_path_buf();
        }
    }
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
