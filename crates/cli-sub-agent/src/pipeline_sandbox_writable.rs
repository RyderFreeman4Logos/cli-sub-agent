use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use csa_config::ProjectConfig;
use tracing::{info, warn};

pub(crate) fn resolve_and_prepare_writable_sources(
    paths: &[PathBuf],
    project_root: &Path,
    source_label: &str,
) -> Result<Vec<PathBuf>, String> {
    let resolved = csa_resource::isolation_plan::resolve_writable_paths(paths, project_root)
        .map_err(|e| format!("{source_label} validation failed: {e}"))?;

    let mut prepared = Vec::with_capacity(resolved.len());
    for (path, candidate) in paths.iter().zip(resolved.iter()) {
        match candidate.try_exists() {
            Ok(true) => prepared.push(candidate.clone()),
            Ok(false) => prepare_missing_source(path, candidate, source_label, &mut prepared),
            Err(error) => warn!(
                source = source_label,
                path = %candidate.display(),
                error = %error,
                "Skipping writable source because it could not be checked before session launch"
            ),
        }
    }
    Ok(prepared)
}

pub(crate) fn resolve_config_extra_writable_sources(
    config: &ProjectConfig,
    project_root: &Path,
) -> Result<Vec<PathBuf>, String> {
    if config.filesystem_sandbox.extra_writable.is_empty() {
        return Ok(Vec::new());
    }
    resolve_and_prepare_writable_sources(
        &config.filesystem_sandbox.extra_writable,
        project_root,
        "filesystem_sandbox.extra_writable",
    )
}

pub(crate) fn resolve_per_tool_writable_sources(
    config: &ProjectConfig,
    tool_name: &str,
    project_root: &Path,
) -> Result<Option<Vec<PathBuf>>, String> {
    let tool_paths = config
        .tools
        .get(tool_name)
        .and_then(|tool| tool.filesystem_sandbox.as_ref())
        .and_then(|sandbox| sandbox.writable_paths.as_ref());
    if let Some(paths) = tool_paths {
        return resolve_paths_with_extra(config, tool_name, project_root, paths).map(Some);
    }

    if let Some(paths) = config
        .filesystem_sandbox
        .tool_writable_overrides
        .get(tool_name)
    {
        return resolve_paths_with_extra(config, tool_name, project_root, paths).map(Some);
    }

    Ok(None)
}

fn resolve_paths_with_extra(
    config: &ProjectConfig,
    tool_name: &str,
    project_root: &Path,
    paths: &[PathBuf],
) -> Result<Vec<PathBuf>, String> {
    let mut resolved = csa_resource::isolation_plan::resolve_writable_paths(paths, project_root)
        .map_err(|e| format!("Per-tool writable_paths validation failed for '{tool_name}': {e}"))?;
    resolved.extend(resolve_config_extra_writable_sources(config, project_root)?);
    Ok(resolved)
}

fn prepare_missing_source(
    original: &Path,
    candidate: &Path,
    source_label: &str,
    prepared: &mut Vec<PathBuf>,
) {
    if path_looks_like_file(original) {
        match prepare_missing_file_source(candidate, original, source_label) {
            Ok(()) => prepared.push(candidate.to_path_buf()),
            Err(message) => warn!(
                source = source_label,
                path = %candidate.display(),
                reason = %message,
                "Skipping missing writable source because it could not be created"
            ),
        }
        return;
    }

    info!(
        source = source_label,
        path = %candidate.display(),
        "Creating missing writable directory before sandbox launch"
    );
    match std::fs::create_dir_all(candidate) {
        Ok(()) => prepared.push(candidate.to_path_buf()),
        Err(error) => warn!(
            source = source_label,
            path = %candidate.display(),
            error = %error,
            "Skipping missing writable directory because it could not be created"
        ),
    }
}

fn path_looks_like_file(path: &Path) -> bool {
    path.extension().is_some()
}

fn prepare_missing_file_source(
    candidate: &Path,
    original: &Path,
    source_label: &str,
) -> Result<(), String> {
    let parent = candidate.parent().ok_or_else(|| {
        format!(
            "{source_label} path '{}' has no parent directory for file pre-creation.",
            original.display()
        )
    })?;
    std::fs::create_dir_all(parent).map_err(|error| {
        format!(
            "{source_label} path '{}' parent '{}' could not be created before session launch: {error}",
            original.display(),
            parent.display()
        )
    })?;
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(candidate)
        .map(|_| ())
        .map_err(|error| {
            format!(
                "{source_label} path '{}' could not be created before session launch: {error}",
                original.display()
            )
        })
}
