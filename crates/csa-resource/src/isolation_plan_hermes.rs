//! Hermes runtime writable paths with read-only profile/config overlays.

#[cfg(test)]
use std::cell::Cell;
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
const RUNTIME_BACKING: &str = ".csa-runtime";

#[cfg(test)]
thread_local! {
    pub(crate) static AFTER_HERMES_HOME_PINNED: Cell<Option<fn(&Path)>> =
        const { Cell::new(None) };
}

#[cfg(test)]
fn run_after_hermes_home_pinned(hermes_home: &Path) {
    AFTER_HERMES_HOME_PINNED.with(|hook| {
        if let Some(inject) = hook.get() {
            inject(hermes_home);
        }
    });
}

fn child_env<'a>(execution_env: Option<&'a HashMap<String, String>>, key: &str) -> Option<&'a str> {
    execution_env
        .and_then(|env| env.get(key))
        .map(String::as_str)
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

    let (hermes_home, source) = if let Some(value) = child_env(execution_env, "HERMES_HOME") {
        (PathBuf::from(value), "HERMES_HOME")
    } else if let Some(value) = std::env::var_os("HERMES_HOME").filter(|value| !value.is_empty()) {
        (PathBuf::from(value), "HERMES_HOME")
    } else if let Some(value) = child_env(execution_env, "HOME") {
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
        #[cfg(test)]
        run_after_hermes_home_pinned(&hermes_home);
        readable::reject_symlink_leaf_at(&home_fd, RUNTIME_BACKING.as_ref())
            .map_err(|error| runtime_leaf_error(&hermes_home.join(RUNTIME_BACKING), error))?;
        let runtime_home_fd =
            readable::open_or_create_writable_dir_at(&home_fd, RUNTIME_BACKING.as_ref())
                .map_err(overlay_protection_error)?;
        let runtime_home = real_home.join(RUNTIME_BACKING);
        writable_paths.push(hermes_home.clone());
        readable_paths.push(ReadablePath::pinned_writable_from(
            hermes_home.clone(),
            runtime_home,
            runtime_home_fd
                .try_clone()
                .map_err(overlay_protection_error)?,
        ));
        let logs_fd = readable::open_or_create_writable_dir_at(&home_fd, "logs".as_ref())
            .map_err(|error| runtime_leaf_error(&logs, error))?;
        writable_paths.push(logs.clone());
        readable_paths.push(ReadablePath::pinned_writable(logs, logs_fd));
        for name in SQLITE_SIDECARS {
            readable::reject_symlink_leaf_at(&home_fd, name.as_ref())
                .map_err(|error| runtime_leaf_error(&hermes_home.join(name), error))?;
            readable::reject_symlink_leaf_at(&runtime_home_fd, name.as_ref())
                .map_err(|error| runtime_leaf_error(&hermes_home.join(name), error))?;
        }
        let names = readable::directory_entry_names(&home_fd)
            .inspect_err(|_| rollback(writable_paths, readable_paths))
            .map_err(|error| overlay_enumeration_error(&hermes_home, error))?;
        let mut protected = Vec::new();
        for name in &names {
            let name_lossy = name.to_string_lossy();
            if name_lossy == "logs"
                || name_lossy == RUNTIME_BACKING
                || name_lossy.starts_with("state.db")
            {
                continue;
            }
            let file = readable::open_overlay_leaf_at(&home_fd, name).map_err(|error| {
                rollback(writable_paths, readable_paths);
                overlay_protection_error(error)
            })?;
            protected.push(ReadablePath::pinned_readonly_overlay(
                hermes_home.join(name),
                real_home.join(name),
                file,
            ));
        }
        for (name, directory) in [("config.yaml", false), ("profiles", true)] {
            if names.iter().any(|candidate| candidate == name) {
                continue;
            }
            let placeholder_name = format!(".csa-absent-{name}-{}", std::process::id());
            let file = readable::create_unlinked_overlay_leaf_at(
                &runtime_home_fd,
                placeholder_name.as_ref(),
                directory,
            )
            .map_err(overlay_protection_error)?;
            protected.push(ReadablePath::pinned_readonly_overlay(
                hermes_home.join(name),
                PathBuf::from("<unlinked>"),
                file,
            ));
        }
        readable_paths.extend(protected);
    }
    #[cfg(not(unix))]
    {
        let _ = home_overlay;
        anyhow::bail!(
            "hermes sandbox preflight failed: pinning writable Hermes runtime leaves requires unix"
        );
    }
    Ok(())
}
