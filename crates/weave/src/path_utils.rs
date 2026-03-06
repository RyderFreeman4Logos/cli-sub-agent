use std::path::{Path, PathBuf};

/// Resolve a symlink target against the directory that contains the link.
pub(crate) fn resolve_symlink_target(link_parent: &Path, target: &Path) -> PathBuf {
    if target.is_absolute() {
        target.to_path_buf()
    } else {
        link_parent.join(target)
    }
}

/// Normalize a path by resolving `.` and `..` components lexically (no I/O).
pub(crate) fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            part => normalized.push(part.as_os_str()),
        }
    }
    normalized
}
