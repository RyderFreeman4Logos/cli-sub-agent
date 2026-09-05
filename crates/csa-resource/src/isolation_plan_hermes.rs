//! Hermes runtime writable paths with read-only profile/config overlays.

#[cfg(test)]
use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs;
#[cfg(unix)]
use std::fs::File;
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
const RUNTIME_READY: &str = ".csa-runtime-ready";

#[cfg(unix)]
#[path = "isolation_plan_hermes_runtime_ready.rs"]
mod hermes_runtime_ready;
#[cfg(unix)]
#[path = "isolation_plan_hermes_sqlite.rs"]
mod hermes_sqlite;

#[cfg(all(unix, test))]
pub(crate) use hermes_runtime_ready::FAIL_DIRECTORY_FSYNC;
#[cfg(all(unix, test))]
pub(crate) use hermes_sqlite::acquire_sqlite_generation_lock;
#[cfg(unix)]
pub(super) use hermes_sqlite::migrate_sqlite_generation;
#[cfg(all(unix, test))]
pub(crate) use hermes_sqlite::{
    AFTER_SQLITE_DESTINATION_OBSERVED, AFTER_SQLITE_NAMED_SNAPSHOT_CREATED,
    AFTER_SQLITE_SNAPSHOT_CREATED, AFTER_SQLITE_SOURCE_OPENED,
};

fn state_db_candidates(root: &Path, hermes_profile: Option<&str>) -> Vec<PathBuf> {
    let Some(profile) = hermes_profile.filter(|value| !value.trim().is_empty()) else {
        return vec![root.join("state.db")];
    };
    let profile = profile.trim();
    vec![
        root.join(profile).join("state.db"),
        root.join("profiles").join(profile).join("state.db"),
        root.join(format!("state.{profile}.db")),
    ]
}

/// Resolve the Hermes `state.db` used by sandbox start/restore, `xurl threads`, and `recall`.
///
/// The authoritative location is `$HERMES_HOME/.csa-runtime/` after a complete
/// preflight (`.csa-runtime-ready` present). Incomplete generations fall back
/// to legacy `$HERMES_HOME/` paths. Callers must not delete the legacy database.
pub fn resolve_hermes_state_db(hermes_home: &Path, hermes_profile: Option<&str>) -> PathBuf {
    if hermes_home.is_file() {
        return hermes_home.to_path_buf();
    }
    let runtime_home = hermes_home.join(RUNTIME_BACKING);
    let runtime_active = runtime_ready(&runtime_home.join(RUNTIME_READY));
    let mut candidates = if runtime_active {
        state_db_candidates(&runtime_home, hermes_profile)
    } else {
        Vec::new()
    };
    let authoritative = candidates.first().cloned().unwrap_or_else(|| {
        if runtime_active {
            runtime_home.join("state.db")
        } else {
            hermes_home.join("state.db")
        }
    });
    let configured_nested_profile = hermes_profile
        .filter(|profile| !profile.trim().is_empty())
        .map(str::trim)
        .filter(|profile| {
            let runtime_direct = runtime_home.join(profile);
            let legacy_direct = hermes_home.join(profile).join("config.yaml");
            let runtime_nested = runtime_home.join("profiles").join(profile);
            let legacy_nested = hermes_home
                .join("profiles")
                .join(profile)
                .join("config.yaml");
            (runtime_nested.is_dir() || legacy_nested.is_file())
                && !runtime_direct.is_dir()
                && !legacy_direct.is_file()
        })
        .map(|profile| {
            let root = if runtime_active {
                &runtime_home
            } else {
                hermes_home
            };
            root.join("profiles").join(profile).join("state.db")
        });
    candidates.extend(state_db_candidates(hermes_home, hermes_profile));
    candidates
        .into_iter()
        .find(|path| path.exists())
        .or(configured_nested_profile)
        .unwrap_or(authoritative)
}

#[cfg(unix)]
fn migrate_profile_directory(
    source_parent: &File,
    destination_parent: &File,
    coordination_parent: &File,
    name: &OsStr,
    destination_path: &Path,
) -> anyhow::Result<Option<File>> {
    let source_profile = match readable::open_directory_at(source_parent, name) {
        Ok(directory) => directory,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound
                    | std::io::ErrorKind::NotADirectory
                    | std::io::ErrorKind::InvalidInput
            ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    let database = readable::open_pinned_regular_at(&source_profile, OsStr::new("state.db"))?;
    if database.is_none()
        && readable::open_pinned_regular_at(&source_profile, OsStr::new("config.yaml"))?.is_none()
    {
        return Ok(None);
    }
    let destination_profile = readable::open_or_create_writable_dir_at(destination_parent, name)
        .map_err(|error| runtime_backing_error(destination_path, error))?;
    if let Some(database) = database {
        migrate_sqlite_generation(
            &source_profile,
            &destination_profile,
            coordination_parent,
            OsStr::new("state.db"),
            database,
            destination_path,
        )?;
    }
    Ok(Some(destination_profile))
}

#[cfg(unix)]
fn migrate_legacy_sqlite(
    home_fd: &File,
    runtime_home_fd: &File,
    names: &[OsString],
    hermes_home: &Path,
) -> anyhow::Result<Vec<(PathBuf, PathBuf, File)>> {
    let mut writable_profiles = Vec::new();
    if let Some(database) = readable::open_pinned_regular_at(home_fd, OsStr::new("state.db"))? {
        migrate_sqlite_generation(
            home_fd,
            runtime_home_fd,
            home_fd,
            OsStr::new("state.db"),
            database,
            &hermes_home.join(RUNTIME_BACKING).join("state.db"),
        )?;
    }
    for name in names {
        if name == OsStr::new(RUNTIME_BACKING) {
            continue;
        }
        if name.to_string_lossy().starts_with("state.") && name.to_string_lossy().ends_with(".db") {
            let Some(database) = readable::open_pinned_regular_at(home_fd, name)? else {
                continue;
            };
            migrate_sqlite_generation(
                home_fd,
                runtime_home_fd,
                home_fd,
                name,
                database,
                &hermes_home.join(RUNTIME_BACKING).join(name),
            )?;
        }
        if let Some(destination_profile) = migrate_profile_directory(
            home_fd,
            runtime_home_fd,
            home_fd,
            name,
            &hermes_home.join(RUNTIME_BACKING).join(name),
        )? {
            writable_profiles.push((
                hermes_home.join(name),
                hermes_home.join(RUNTIME_BACKING).join(name),
                destination_profile,
            ));
        }
    }
    let profiles = match readable::open_directory_at(home_fd, OsStr::new("profiles")) {
        Ok(directory) => directory,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound
                    | std::io::ErrorKind::NotADirectory
                    | std::io::ErrorKind::InvalidInput
            ) =>
        {
            return Ok(writable_profiles);
        }
        Err(error) => return Err(error.into()),
    };
    let profile_names = readable::directory_entry_names(&profiles)
        .map_err(|error| overlay_enumeration_error(&hermes_home.join("profiles"), error))?;
    let mut destination_profiles = None;
    for name in profile_names {
        let source_profile = match readable::open_directory_at(&profiles, &name) {
            Ok(directory) => directory,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound
                        | std::io::ErrorKind::NotADirectory
                        | std::io::ErrorKind::InvalidInput
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        let database = readable::open_pinned_regular_at(&source_profile, OsStr::new("state.db"))?;
        if database.is_none()
            && readable::open_pinned_regular_at(&source_profile, OsStr::new("config.yaml"))?
                .is_none()
        {
            continue;
        }
        if destination_profiles.is_none() {
            destination_profiles = Some(
                readable::open_or_create_writable_dir_at(runtime_home_fd, OsStr::new("profiles"))
                    .map_err(|error| {
                        runtime_backing_error(&hermes_home.join(RUNTIME_BACKING), error)
                    }),
            );
        }
        let destination_profiles = match destination_profiles.as_ref() {
            Some(Ok(directory)) => directory,
            Some(Err(error)) => return Err(anyhow::anyhow!("{error}")),
            None => unreachable!("destination profiles is initialized above"),
        };
        let destination_path = hermes_home
            .join(RUNTIME_BACKING)
            .join("profiles")
            .join(&name);
        let destination_profile =
            readable::open_or_create_writable_dir_at(destination_profiles, &name)
                .map_err(|error| runtime_backing_error(&destination_path, error))?;
        if let Some(database) = database {
            migrate_sqlite_generation(
                &source_profile,
                &destination_profile,
                home_fd,
                OsStr::new("state.db"),
                database,
                &destination_path.join("state.db"),
            )?;
        }
        writable_profiles.push((
            hermes_home.join("profiles").join(&name),
            destination_path,
            destination_profile,
        ));
    }
    Ok(writable_profiles)
}

#[cfg(not(unix))]
fn migrate_legacy_sqlite(
    _home_fd: &(),
    _runtime_home_fd: &(),
    _names: &[OsString],
    _hermes_home: &Path,
) -> anyhow::Result<()> {
    Ok(())
}

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

fn runtime_backing_error(leaf: &Path, error: std::io::Error) -> anyhow::Error {
    anyhow::anyhow!(
        "hermes sandbox preflight failed: runtime backing path {} is not writable: {error}",
        leaf.display()
    )
}

fn runtime_ready(marker: &Path) -> bool {
    std::fs::symlink_metadata(marker).is_ok_and(|meta| meta.file_type().is_file())
}

pub(super) fn finish_plan(
    filesystem: FilesystemCapability,
    bind_fd_count: usize,
    pending: Option<PendingRuntimeActivation>,
) -> anyhow::Result<()> {
    if filesystem == FilesystemCapability::Bwrap
        && bind_fd_count > 0
        && !crate::filesystem_sandbox::has_bwrap_bind_fd_options()
    {
        anyhow::bail!(
            "sandbox preflight failed: bwrap bind-fd required but --ro-bind-fd/--bind-fd are unavailable"
        );
    }
    #[cfg(unix)]
    if let (FilesystemCapability::Bwrap, Some(pending)) = (filesystem, pending) {
        pending.publish().map_err(|error| {
            anyhow::anyhow!(
                "hermes sandbox preflight failed: runtime activation marker is not writable: {error}"
            )
        })?;
    }
    #[cfg(not(unix))]
    let _ = pending;
    Ok(())
}

#[cfg(unix)]
#[derive(Debug)]
pub(super) struct PendingRuntimeActivation {
    runtime_home_fd: File,
    marker_path: PathBuf,
    database_paths: Vec<PathBuf>,
}

#[cfg(unix)]
impl PendingRuntimeActivation {
    pub(super) fn publish(self) -> std::io::Result<()> {
        hermes_runtime_ready::activate_runtime_generation(
            &self.runtime_home_fd,
            &self.database_paths,
        )
        .map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!("{}: {error}", self.marker_path.display()),
            )
        })
    }
}

#[cfg(not(unix))]
#[derive(Debug)]
pub(super) struct PendingRuntimeActivation;

pub(super) fn add_hermes_runtime_paths(
    filesystem: FilesystemCapability,
    home: Option<&Path>,
    execution_env: Option<&HashMap<String, String>>,
    writable_paths: &mut Vec<PathBuf>,
    readable_paths: &mut Vec<ReadablePath>,
    required_writable_dirs: &mut Vec<RequiredWritableDir>,
) -> anyhow::Result<Option<PendingRuntimeActivation>> {
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
        return Ok(None);
    };
    if !hermes_home.is_absolute() {
        anyhow::bail!("hermes sandbox preflight failed: {source} must be an absolute path");
    }
    if hermes_home == Path::new("/tmp") {
        anyhow::bail!("hermes sandbox preflight failed: {source} must not be /tmp itself");
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
        return Ok(None);
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
        readable::recover_reserved_names_at(&home_fd)
            .map_err(|error| overlay_enumeration_error(&hermes_home, error))?;
        let runtime_home = real_home.join(RUNTIME_BACKING);
        let runtime_home_fd =
            readable::open_or_create_writable_dir_at(&home_fd, RUNTIME_BACKING.as_ref())
                .map_err(|error| runtime_backing_error(&runtime_home, error))?;
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
        let migrated_profiles =
            migrate_legacy_sqlite(&home_fd, &runtime_home_fd, &names, &hermes_home)?;
        let mut database_paths = vec![hermes_home.join(RUNTIME_BACKING).join("state.db")];
        database_paths.extend(
            migrated_profiles
                .iter()
                .map(|(_, bind_source, _)| bind_source.join("state.db")),
        );
        let migrated_profile_requests: Vec<PathBuf> = migrated_profiles
            .iter()
            .map(|(requested, _, _)| requested.clone())
            .collect();
        for (requested, bind_source, file) in migrated_profiles {
            writable_paths.push(requested.clone());
            readable_paths.push(ReadablePath::pinned_writable_from(
                requested,
                bind_source,
                file,
            ));
        }
        let flat_profile_bases: Vec<OsString> = names
            .iter()
            .filter(|name| {
                let name = name.to_string_lossy();
                name.strip_prefix("state.")
                    .is_some_and(|profile| profile.ends_with(".db"))
            })
            .cloned()
            .collect();
        let mut protected = Vec::new();
        for name in &names {
            let name_lossy = name.to_string_lossy();
            let is_flat_profile_generation = flat_profile_bases.iter().any(|base| {
                name == base || name_lossy.starts_with(&format!("{}-", base.to_string_lossy()))
            });
            if name_lossy == "logs"
                || name_lossy == RUNTIME_BACKING
                || name_lossy == RUNTIME_READY
                || name_lossy == ".csa-sqlite-generation.lock"
                || name_lossy == ".csa-reserved-name.lock"
                || name_lossy == ".csa-sqlite-staging"
                || name_lossy.starts_with("state.db")
                || is_flat_profile_generation
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
        for requested in migrated_profile_requests {
            let relative = requested.strip_prefix(&hermes_home).map_err(|_| {
                overlay_protection_error(std::io::Error::other(
                    "migrated Hermes profile is outside Hermes home",
                ))
            })?;
            let config = requested.join("config.yaml");
            let config_source = real_home.join(relative).join("config.yaml");
            if config_source.exists() {
                let overlay =
                    ReadablePath::try_pinned_readonly_overlay_from(config.clone(), config_source)
                        .map_err(overlay_protection_error)?;
                let file = overlay
                    .open_pinned_regular_file()
                    .map_err(overlay_protection_error)?;
                protected.push(ReadablePath::pinned_readonly_overlay(
                    config,
                    real_home.join(relative).join("config.yaml"),
                    file,
                ));
            }
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
        Ok(Some(PendingRuntimeActivation {
            runtime_home_fd,
            marker_path: hermes_home.join(RUNTIME_BACKING).join(RUNTIME_READY),
            database_paths,
        }))
    }
    #[cfg(not(unix))]
    {
        let _ = home_overlay;
        anyhow::bail!(
            "hermes sandbox preflight failed: pinning writable Hermes runtime leaves requires unix"
        );
    }
}
