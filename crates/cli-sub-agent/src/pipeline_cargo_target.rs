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

    pub(crate) fn requires_sandbox_writeability_validation(&self) -> bool {
        self.policy_reason != "not_applicable"
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
        let lexical_target = cargo_target_lexical_path(&explicit_target, project_root);
        let target_status = match target_writeability(&lexical_target, CargoTargetSource::Explicit)
        {
            TargetWriteability::Writable { status } => status,
            TargetWriteability::Unavailable { status, error }
            | TargetWriteability::Unwritable { status, error } => {
                return Err(cargo_target_preflight_error(
                    &lexical_target,
                    &status,
                    error.as_deref(),
                ));
            }
        };
        info!(
            project_target = %workspace_target.display(),
            selected_cargo_target = %lexical_target.display(),
            selected_cargo_target_status = %target_status,
            tool = tool_name,
            "Run session: explicit CARGO_TARGET_DIR preserved"
        );
        return Ok(CargoTargetPolicyReport::new(
            &workspace_target,
            &explicit_target,
            "explicit_override_preserved",
            target_status,
            None,
            true,
            false,
        ));
    }

    match workspace_target_writeability(&workspace_target) {
        TargetWriteability::Writable { status } => {
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
        TargetWriteability::Unavailable { status, error }
        | TargetWriteability::Unwritable { status, error } => Err(cargo_target_preflight_error(
            &workspace_target,
            &status,
            error.as_deref(),
        )),
    }
}

pub(crate) fn ensure_cargo_target_sandbox_writable(
    report: &CargoTargetPolicyReport,
    project_root: &Path,
    isolation_plan: Option<&csa_resource::isolation_plan::IsolationPlan>,
) -> Result<(), String> {
    if !report.requires_sandbox_writeability_validation() {
        return Ok(());
    }
    let lexical_target = cargo_target_lexical_path(&report.selected_cargo_target, project_root);
    let source = if report.explicit_override_preserved {
        CargoTargetSource::Explicit
    } else {
        CargoTargetSource::Workspace
    };
    match target_writeability(&lexical_target, source) {
        TargetWriteability::Writable { .. } => {}
        TargetWriteability::Unavailable { status, error }
        | TargetWriteability::Unwritable { status, error } => {
            return Err(cargo_target_preflight_error(
                &lexical_target,
                &status,
                error.as_deref(),
            ));
        }
    }

    let Some(plan) =
        isolation_plan.filter(|plan| plan.filesystem != csa_resource::FilesystemCapability::None)
    else {
        return Ok(());
    };
    let resolved_target = resolved_target_path_for_diagnostics(&lexical_target);
    let resolved_project_root = plan
        .readonly_project_root
        .then(|| {
            csa_resource::isolation_plan::canonicalize_through_existing_ancestors(project_root).ok()
        })
        .flatten();
    let is_granted = plan.writable_paths.iter().any(|writable_path| {
        csa_resource::isolation_plan::canonicalize_through_existing_ancestors(writable_path)
            .is_ok_and(|resolved_writable_path| {
                let generic_readonly_project_root = plan.readonly_project_root
                    && (writable_path == project_root
                        || resolved_project_root
                            .as_ref()
                            .is_some_and(|root| root == &resolved_writable_path));
                !generic_readonly_project_root
                    && resolved_target.starts_with(resolved_writable_path)
            })
    });
    if is_granted {
        return Ok(());
    }

    Err(format!(
        "Cargo target preflight blocked before provider execution: configured target lexical path \
         '{}' resolves to '{}'; status 'workspace_target_not_granted_by_sandbox'. The resolved \
         target is writable on the host but is not granted by the final filesystem sandbox plan. \
         Add the resolved path to filesystem_sandbox.extra_writable or the tool's writable_paths \
         before retrying.",
        lexical_target.display(),
        resolved_target.display(),
    ))
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

fn cargo_target_lexical_path(target: impl AsRef<Path>, project_root: &Path) -> PathBuf {
    let target = target.as_ref();
    if target.is_absolute() {
        target.to_path_buf()
    } else {
        project_root.join(target)
    }
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

#[derive(Clone, Copy)]
enum CargoTargetSource {
    Workspace,
    Explicit,
}

impl CargoTargetSource {
    fn status(self, suffix: &str) -> String {
        let prefix = match self {
            Self::Workspace => "workspace_target",
            Self::Explicit => "explicit_target",
        };
        format!("{prefix}_{suffix}")
    }
}

enum TargetWriteability {
    Writable {
        status: String,
    },
    Unavailable {
        status: String,
        error: Option<String>,
    },
    Unwritable {
        status: String,
        error: Option<String>,
    },
}

fn workspace_target_writeability(workspace_target: &Path) -> TargetWriteability {
    target_writeability(workspace_target, CargoTargetSource::Workspace)
}

fn target_writeability(target: &Path, source: CargoTargetSource) -> TargetWriteability {
    let mut target_absent = false;
    let probe_dir = match std::fs::symlink_metadata(target) {
        Ok(metadata) if metadata.file_type().is_symlink() => match std::fs::metadata(target) {
            Ok(target_metadata) if target_metadata.is_dir() => {
                resolved_target_path_for_diagnostics(target)
            }
            Ok(_) => {
                return TargetWriteability::Unavailable {
                    status: source.status("symlink_not_directory"),
                    error: None,
                };
            }
            Err(error) => {
                return TargetWriteability::Unavailable {
                    status: source.status("symlink_unavailable"),
                    error: Some(error.to_string()),
                };
            }
        },
        Ok(metadata) if metadata.is_dir() => resolved_target_path_for_diagnostics(target),
        Ok(_) => {
            return TargetWriteability::Unavailable {
                status: source.status("not_directory"),
                error: None,
            };
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            target_absent = true;
            match nearest_existing_directory(target) {
                Ok(parent) => parent,
                Err(error) => {
                    return TargetWriteability::Unavailable {
                        status: source.status("missing_parent_unavailable"),
                        error: Some(error),
                    };
                }
            }
        }
        Err(error) => {
            return TargetWriteability::Unavailable {
                status: source.status("metadata_error"),
                error: Some(error.to_string()),
            };
        }
    };

    match writable_directory_probe(&probe_dir) {
        Ok(()) => TargetWriteability::Writable {
            status: if target_absent && matches!(source, CargoTargetSource::Workspace) {
                "workspace_target_absent_cargo_default".to_string()
            } else {
                source.status("writable")
            },
        },
        Err(error) => TargetWriteability::Unwritable {
            status: if target_absent {
                source.status("missing_parent_unwritable")
            } else {
                source.status("unwritable")
            },
            error: Some(error),
        },
    }
}

fn nearest_existing_directory(target: &Path) -> Result<PathBuf, String> {
    let mut candidate = target.to_path_buf();
    loop {
        match std::fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let target_metadata = std::fs::metadata(&candidate)
                    .map_err(|error| format!("{}: {error}", candidate.display()))?;
                if !target_metadata.is_dir() {
                    return Err(format!("{} is not a directory", candidate.display()));
                }
                return candidate
                    .canonicalize()
                    .map_err(|error| format!("{}: {error}", candidate.display()));
            }
            Ok(metadata) if metadata.is_dir() => {
                return candidate
                    .canonicalize()
                    .map_err(|error| format!("{}: {error}", candidate.display()));
            }
            Ok(_) => return Err(format!("{} is not a directory", candidate.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !candidate.pop() {
                    return Err(format!("no existing parent for {}", target.display()));
                }
            }
            Err(error) => return Err(format!("{}: {error}", candidate.display())),
        }
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
