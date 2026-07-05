use crate::filesystem_sandbox::FilesystemCapability;

use std::path::{Component, Path, PathBuf};

pub(super) const DEFAULT_SANDBOX_TMPDIR: &str = "/tmp";

pub(super) fn runtime_daemon_socket_paths(runtime_root: &Path) -> [PathBuf; 2] {
    [
        runtime_root.join("bus"),
        runtime_root.join("systemd/private"),
    ]
}

pub(super) fn sandbox_tmpdir_for_capability(
    filesystem: FilesystemCapability,
    session_dir: &Path,
) -> PathBuf {
    match filesystem {
        FilesystemCapability::Bwrap => PathBuf::from(DEFAULT_SANDBOX_TMPDIR),
        FilesystemCapability::Landlock | FilesystemCapability::None => session_dir.join("tmp"),
    }
}

pub(super) fn canonicalize_or_fallback(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

pub(super) fn xdg_runtime_root() -> Option<PathBuf> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)?;
    if !runtime_dir.is_absolute() {
        return None;
    }
    let normalized = normalize_path_components(runtime_dir);
    if normalized == Path::new("/") {
        return None;
    }
    Some(canonicalize_or_fallback(&normalized))
}

pub(super) fn is_xdg_runtime_child_path(path: &Path) -> bool {
    xdg_runtime_root()
        .as_ref()
        .is_some_and(|root| path.starts_with(root) && path != root)
}

pub(super) fn normalize_path_components(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized.as_os_str().is_empty() || normalized == Path::new("/") {
                    continue;
                }
                normalized.pop();
            }
        }
    }
    normalized
}

/// Portable home-directory lookup (avoids pulling in the `dirs` crate).
pub(super) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Detect whether `project_root` is inside a git submodule and return the
/// superproject root if so.
pub(super) fn detect_superproject_root(project_root: &Path) -> Option<PathBuf> {
    let dot_git = project_root.join(".git");

    if !dot_git.is_file() {
        return None;
    }

    for ancestor in project_root.ancestors().skip(1) {
        if ancestor.join(".git").is_dir() {
            return Some(ancestor.to_path_buf());
        }
    }

    None
}

/// Reject paths under sensitive system directories that should never be
/// writable inside a sandbox.  Allows legitimate paths like home dirs,
/// `/tmp`, `/usr/local/share/mise`, etc.
pub(super) fn is_sensitive_system_path(path: &Path) -> bool {
    const SENSITIVE_PREFIXES: &[&str] = &[
        "/etc",
        "/var/lib",
        "/var/log",
        "/var/run",
        "/boot",
        "/sbin",
        "/bin",
        "/lib",
        "/lib64",
        "/sys",
        "/proc",
        "/dev",
        "/run",
        "/private/etc",
        "/private/var/lib",
        "/private/var/log",
        "/private/var/run",
    ];

    for prefix in SENSITIVE_PREFIXES {
        if path.starts_with(prefix) {
            return true;
        }
    }
    path == Path::new("/")
}

/// Add `dir` to `paths` if it exists, otherwise pre-create it when a
/// non-root ancestor exists (bwrap `--bind` requires the source path to exist).
///
/// Rejects paths under sensitive system directories (`/etc`, `/var/lib`,
/// `/boot`, `/sbin`, etc.) to prevent env vars like `CARGO_HOME` from
/// escaping the sandbox boundary.
pub(super) fn add_dir_or_creatable_parent(paths: &mut Vec<PathBuf>, dir: &Path) -> bool {
    if is_sensitive_system_path(dir) {
        tracing::warn!(
            path = %dir.display(),
            "rejecting writable path under sensitive system directory"
        );
        return false;
    }

    if dir.exists() {
        paths.push(dir.to_path_buf());
        true
    } else if dir
        .ancestors()
        .skip(1)
        .find(|ancestor| ancestor.exists())
        .is_some_and(|ancestor| ancestor != Path::new("/"))
    {
        // Pre-create the directory so bwrap --bind can mount it.
        // On cold starts (fresh CARGO_HOME/RUSTUP_HOME/shared npm cache) the
        // dir or one of its intermediate parents won't exist yet; bwrap
        // requires the source path to be present.
        match std::fs::create_dir_all(dir) {
            Ok(()) => {
                paths.push(dir.to_path_buf());
                true
            }
            Err(e) => {
                tracing::warn!(
                    path = %dir.display(),
                    error = %e,
                    "failed to pre-create directory for sandbox writable mount, skipping"
                );
                false
            }
        }
    } else {
        false
    }
}
