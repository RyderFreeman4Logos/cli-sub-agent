//! Hermes runtime writable paths with read-only profile/config overlays.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::filesystem_sandbox::FilesystemCapability;

use super::codex_paths::RequiredWritableDir;
use super::{ReadablePath, add_dir_or_creatable_parent};

const SQLITE_SIDECARS: [&str; 4] = [
    "state.db",
    "state.db-wal",
    "state.db-shm",
    "state.db-journal",
];

fn nonempty_env<'a>(
    execution_env: Option<&'a HashMap<String, String>>,
    key: &str,
) -> Option<&'a str> {
    execution_env
        .and_then(|env| env.get(key))
        .map(String::as_str)
        .filter(|value| !value.is_empty())
}

fn overlay_enumeration_error(hermes_home: &Path, error: std::io::Error) -> anyhow::Error {
    anyhow::anyhow!(
        "hermes sandbox preflight failed: cannot enumerate Hermes configuration overlays in {}: {error}",
        hermes_home.display()
    )
}

fn overlay_protection_error(error: std::io::Error) -> anyhow::Error {
    anyhow::anyhow!(
        "hermes sandbox preflight failed: cannot protect Hermes configuration overlay: {error}"
    )
}

fn reject_runtime_leaf_symlink(hermes_home: &Path, leaf: &Path) -> anyhow::Result<()> {
    match fs::symlink_metadata(leaf) {
        Ok(metadata) if metadata.file_type().is_symlink() => anyhow::bail!(
            "hermes sandbox preflight failed: runtime path {} is a symlink",
            leaf.display()
        ),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(overlay_enumeration_error(hermes_home, error)),
    }
}

pub(super) fn add_hermes_runtime_paths(
    filesystem: FilesystemCapability,
    home: Option<&Path>,
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

    let (hermes_home, source) = if let Some(value) = nonempty_env(execution_env, "HERMES_HOME") {
        (PathBuf::from(value), "HERMES_HOME")
    } else if let Some(value) = std::env::var_os("HERMES_HOME").filter(|value| !value.is_empty()) {
        (PathBuf::from(value), "HERMES_HOME")
    } else if let Some(value) = nonempty_env(execution_env, "HOME") {
        (PathBuf::from(value).join(".hermes"), "HOME/.hermes")
    } else if let Some(home) = home {
        (home.join(".hermes"), "HOME/.hermes")
    } else {
        return Ok(());
    };
    if !hermes_home.is_absolute() {
        anyhow::bail!("hermes sandbox preflight failed: {source} must be an absolute path");
    }
    let logs = hermes_home.join("logs");
    required_writable_dirs.push(RequiredWritableDir {
        path: logs.clone(),
        source: source.to_string(),
        purpose: "Hermes logs and SQLite state database",
        config_hint: "HERMES_HOME",
        tool_label: "hermes",
    });

    if filesystem != FilesystemCapability::Bwrap {
        return Ok(());
    }

    let writable_start = writable_paths.len();
    reject_runtime_leaf_symlink(&hermes_home, &logs)?;
    if !add_dir_or_creatable_parent(writable_paths, &logs) {
        return Ok(());
    }

    let real_home = fs::canonicalize(&hermes_home)
        .map_err(|error| overlay_enumeration_error(&hermes_home, error))?;
    for name in SQLITE_SIDECARS {
        let sidecar = hermes_home.join(name);
        reject_runtime_leaf_symlink(&hermes_home, &sidecar)?;
        if !sidecar.exists() {
            fs::write(&sidecar, b"")
                .map_err(|error| overlay_enumeration_error(&hermes_home, error))?;
        }
        writable_paths.push(sidecar);
    }

    let home_overlay =
        ReadablePath::try_pinned_readonly_overlay_from(hermes_home.clone(), real_home.clone())
            .inspect_err(|_| writable_paths.truncate(writable_start))
            .map_err(overlay_protection_error)?;
    let entries = fs::read_dir(&real_home)
        .inspect_err(|_| writable_paths.truncate(writable_start))
        .map_err(|error| overlay_enumeration_error(&hermes_home, error))?;
    let mut protected = vec![home_overlay];
    for entry in entries {
        let entry = entry.map_err(|error| {
            writable_paths.truncate(writable_start);
            overlay_enumeration_error(&hermes_home, error)
        })?;
        let name = entry.file_name();
        let name_lossy = name.to_string_lossy();
        if name_lossy == "logs" || name_lossy.starts_with("state.db") {
            continue;
        }
        let overlay = ReadablePath::try_pinned_readonly_overlay_from(
            hermes_home.join(&name),
            real_home.join(&name),
        )
        .map_err(|error| {
            writable_paths.truncate(writable_start);
            overlay_protection_error(error)
        })?;
        protected.push(overlay);
    }
    readable_paths.extend(protected);
    Ok(())
}
