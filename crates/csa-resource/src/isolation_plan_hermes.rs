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
/// The authoritative location is `$HERMES_HOME/.csa-runtime/` (plus profile candidates).
/// Legacy `$HERMES_HOME/` paths are discovered when the runtime copy is absent.
/// Callers must not delete the legacy database.
pub fn resolve_hermes_state_db(hermes_home: &Path, hermes_profile: Option<&str>) -> PathBuf {
    if hermes_home.is_file() {
        return hermes_home.to_path_buf();
    }
    let runtime_home = hermes_home.join(RUNTIME_BACKING);
    let mut candidates = state_db_candidates(&runtime_home, hermes_profile);
    let authoritative = candidates
        .first()
        .cloned()
        .unwrap_or_else(|| runtime_home.join("state.db"));
    candidates.extend(state_db_candidates(hermes_home, hermes_profile));
    candidates
        .into_iter()
        .find(|path| path.exists())
        .unwrap_or(authoritative)
}

#[cfg(unix)]
fn sqlite_generation_names(base: &OsStr) -> Vec<OsString> {
    let mut names = Vec::with_capacity(SQLITE_SIDECARS.len());
    names.push(base.to_os_string());
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut name = base.to_os_string();
        name.push(suffix);
        names.push(name);
    }
    names
}

#[cfg(unix)]
fn is_regular(stat: &libc::stat) -> bool {
    stat.st_mode & libc::S_IFMT == libc::S_IFREG
}

#[cfg(unix)]
fn remove_if_present(parent: &File, name: &OsStr) -> anyhow::Result<()> {
    match readable::stat_at(parent, name) {
        Ok(stat) if is_regular(&stat) => readable::remove_file_at(parent, name)?,
        Ok(_) => anyhow::bail!("SQLite generation member is not a regular file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

#[cfg(unix)]
fn runtime_generation_present(parent: &File, base: &OsStr) -> anyhow::Result<bool> {
    let names = sqlite_generation_names(base);
    let database_present = match readable::stat_at(parent, &names[0]) {
        Ok(stat) if is_regular(&stat) => true,
        Ok(_) => anyhow::bail!("SQLite state database is not a regular file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    for name in names.iter().skip(1) {
        match readable::stat_at(parent, name) {
            Ok(stat) if is_regular(&stat) => {
                if !database_present {
                    readable::remove_file_at(parent, name)?;
                }
            }
            Ok(_) => anyhow::bail!("SQLite generation member is not a regular file"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(database_present)
}

#[cfg(unix)]
fn cleanup_runtime_generation(parent: &File, base: &OsStr) {
    for name in sqlite_generation_names(base) {
        let _ = remove_if_present(parent, &name);
    }
}

#[cfg(unix)]
fn migrate_sqlite_generation(
    source_parent: &File,
    destination_parent: &File,
    base: &OsStr,
    source_database: File,
    destination_path: &Path,
) -> anyhow::Result<()> {
    let names = sqlite_generation_names(base);
    let mut source_members = vec![(names[0].clone(), source_database)];
    for name in names.iter().skip(1) {
        if let Some(file) = readable::open_pinned_regular_at(source_parent, name)? {
            source_members.push((name.clone(), file));
        }
    }
    if runtime_generation_present(destination_parent, base)? {
        return Ok(());
    }
    let result = (|| {
        for (name, source) in source_members.iter().skip(1) {
            readable::copy_pinned_file_atomic(source, destination_parent, name)?;
        }
        readable::copy_pinned_file_atomic(&source_members[0].1, destination_parent, &names[0])?;
        Ok(())
    })();
    if let Err(error) = result {
        cleanup_runtime_generation(destination_parent, base);
        return Err(runtime_backing_error(destination_path, error));
    }
    Ok(())
}

#[cfg(unix)]
fn migrate_profile_directory(
    source_parent: &File,
    destination_parent: &File,
    name: &OsStr,
    destination_path: &Path,
) -> anyhow::Result<()> {
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
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    let Some(database) = readable::open_pinned_regular_at(&source_profile, OsStr::new("state.db"))?
    else {
        return Ok(());
    };
    let destination_profile = readable::open_or_create_writable_dir_at(destination_parent, name)
        .map_err(|error| runtime_backing_error(destination_path, error))?;
    migrate_sqlite_generation(
        &source_profile,
        &destination_profile,
        OsStr::new("state.db"),
        database,
        destination_path,
    )
}

#[cfg(unix)]
fn migrate_legacy_sqlite(
    home_fd: &File,
    runtime_home_fd: &File,
    names: &[OsString],
    hermes_home: &Path,
) -> anyhow::Result<()> {
    if let Some(database) = readable::open_pinned_regular_at(home_fd, OsStr::new("state.db"))? {
        migrate_sqlite_generation(
            home_fd,
            runtime_home_fd,
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
                name,
                database,
                &hermes_home.join(RUNTIME_BACKING).join(name),
            )?;
        }
        migrate_profile_directory(
            home_fd,
            runtime_home_fd,
            name,
            &hermes_home.join(RUNTIME_BACKING).join(name),
        )?;
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
            return Ok(());
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
        let Some(database) =
            readable::open_pinned_regular_at(&source_profile, OsStr::new("state.db"))?
        else {
            continue;
        };
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
        migrate_sqlite_generation(
            &source_profile,
            &destination_profile,
            OsStr::new("state.db"),
            database,
            &destination_path.join("state.db"),
        )?;
    }
    Ok(())
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
        migrate_legacy_sqlite(&home_fd, &runtime_home_fd, &names, &hermes_home)?;
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
