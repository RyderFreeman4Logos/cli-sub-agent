//! Hermes runtime writable paths with read-only profile/config overlays.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::filesystem_sandbox::FilesystemCapability;

use super::codex_paths::RequiredWritableDir;
use super::{ReadablePath, add_dir_or_creatable_parent};

pub(super) fn add_hermes_runtime_paths(
    filesystem: FilesystemCapability,
    home: &Path,
    execution_env: Option<&HashMap<String, String>>,
    writable_paths: &mut Vec<PathBuf>,
    readable_paths: &mut Vec<ReadablePath>,
    required_writable_dirs: &mut Vec<RequiredWritableDir>,
) -> anyhow::Result<()> {
    if filesystem == FilesystemCapability::Landlock {
        anyhow::bail!(
            "hermes sandbox preflight failed: Landlock cannot protect read-only Hermes configuration below writable runtime paths"
        );
    }

    let (hermes_home, source) = match execution_env
        .and_then(|env| env.get("HERMES_HOME"))
        .filter(|value| !value.is_empty())
    {
        Some(value) => (PathBuf::from(value), "HERMES_HOME"),
        None => match std::env::var_os("HERMES_HOME") {
            Some(value) if !value.is_empty() => (PathBuf::from(value), "HERMES_HOME"),
            _ => (home.join(".hermes"), "HOME/.hermes"),
        },
    };
    required_writable_dirs.push(RequiredWritableDir {
        path: hermes_home.clone(),
        source: source.to_string(),
        purpose: "Hermes logs and SQLite state database",
        config_hint: "HERMES_HOME",
        tool_label: "hermes",
    });

    if filesystem != FilesystemCapability::Bwrap
        || !add_dir_or_creatable_parent(writable_paths, &hermes_home)
    {
        return Ok(());
    }

    let Ok(entries) = fs::read_dir(&hermes_home) else {
        writable_paths.retain(|path| path != &hermes_home);
        return Ok(());
    };
    let mut protected = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            writable_paths.retain(|path| path != &hermes_home);
            return Ok(());
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "logs" || name.starts_with("state.db") {
            continue;
        }
        let path = entry.path();
        let path = ReadablePath::try_pinned_readonly_overlay(path).map_err(|error| {
            writable_paths.retain(|candidate| candidate != &hermes_home);
            anyhow::anyhow!("hermes sandbox preflight failed: cannot protect Hermes configuration overlay: {error}")
        })?;
        protected.push(path);
    }
    readable_paths.extend(protected);
    Ok(())
}
