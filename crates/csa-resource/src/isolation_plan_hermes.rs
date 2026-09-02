//! Hermes runtime writable paths with read-only profile/config overlays.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::filesystem_sandbox::FilesystemCapability;

use super::ReadablePath;
use super::codex_paths::RequiredWritableDir;
use super::readable;

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

fn runtime_leaf_error(leaf: &Path, error: std::io::Error) -> anyhow::Error {
    anyhow::anyhow!(
        "hermes sandbox preflight failed: runtime path {} is a symlink: {error}",
        leaf.display()
    )
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
    let readable_start = readable_paths.len();
    let rollback = |writable_paths: &mut Vec<PathBuf>, readable_paths: &mut Vec<ReadablePath>| {
        writable_paths.truncate(writable_start);
        readable_paths.truncate(readable_start);
    };

    let real_home = match fs::canonicalize(&hermes_home) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(&hermes_home)
                .map_err(|error| overlay_enumeration_error(&hermes_home, error))?;
            fs::canonicalize(&hermes_home)
                .map_err(|error| overlay_enumeration_error(&hermes_home, error))?
        }
        Err(error) => return Err(overlay_enumeration_error(&hermes_home, error)),
    };
    let home_overlay =
        ReadablePath::try_pinned_readonly_overlay_from(hermes_home.clone(), real_home.clone())
            .map_err(overlay_protection_error)?;

    #[cfg(unix)]
    {
        let home_fd = home_overlay.pinned_source_file().ok_or_else(|| {
            overlay_protection_error(std::io::Error::other(
                "Hermes home is not a pinned directory",
            ))
        })?;
        let logs_fd = readable::open_or_create_writable_dir_at(&home_fd, "logs".as_ref())
            .map_err(|error| runtime_leaf_error(&logs, error))?;
        writable_paths.push(logs.clone());
        readable_paths.push(ReadablePath::pinned_writable(logs, logs_fd));
        for name in SQLITE_SIDECARS {
            let sidecar = hermes_home.join(name);
            let sidecar_fd = readable::open_or_create_writable_file_at(&home_fd, name.as_ref())
                .map_err(|error| runtime_leaf_error(&sidecar, error))?;
            writable_paths.push(sidecar.clone());
            readable_paths.push(ReadablePath::pinned_writable(sidecar, sidecar_fd));
        }
    }
    #[cfg(not(unix))]
    {
        anyhow::bail!(
            "hermes sandbox preflight failed: pinning writable Hermes runtime leaves requires unix"
        );
    }

    let entries = fs::read_dir(&real_home)
        .inspect_err(|_| rollback(writable_paths, readable_paths))
        .map_err(|error| overlay_enumeration_error(&hermes_home, error))?;
    let mut protected = vec![home_overlay];
    for entry in entries {
        let entry = entry.map_err(|error| {
            rollback(writable_paths, readable_paths);
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
            rollback(writable_paths, readable_paths);
            overlay_protection_error(error)
        })?;
        protected.push(overlay);
    }
    readable_paths.extend(protected);
    Ok(())
}
