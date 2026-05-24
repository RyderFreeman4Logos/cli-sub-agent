use std::path::{Component, Path, PathBuf};

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
        "/etc", "/var/lib", "/var/log", "/var/run", "/boot", "/sbin", "/bin", "/lib", "/lib64",
        "/sys", "/proc", "/dev", "/run",
    ];

    for prefix in SENSITIVE_PREFIXES {
        if path.starts_with(prefix) {
            return true;
        }
    }
    path == Path::new("/")
}
