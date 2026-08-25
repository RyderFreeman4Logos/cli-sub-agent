use std::path::{Path, PathBuf};

use crate::filesystem_sandbox::FilesystemCapability;

use super::runtime_path;

/// Validated readable bind: requested destination plus the source pinned at
/// validation time so later symlink replacement cannot change the bind (#3102).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadablePath {
    requested: PathBuf,
    bind_source: PathBuf,
}

impl ReadablePath {
    /// Requested destination stored for the sandbox mount.
    pub fn requested(&self) -> &Path {
        &self.requested
    }

    /// Bind source pinned when the path was validated or first added.
    pub fn bind_source(&self) -> &Path {
        &self.bind_source
    }

    pub(crate) fn pinned(requested: PathBuf, bind_source: PathBuf) -> Self {
        Self {
            requested,
            bind_source,
        }
    }
}

impl From<PathBuf> for ReadablePath {
    fn from(requested: PathBuf) -> Self {
        let bind_source = runtime_path::canonicalize_or_fallback(&requested);
        Self::pinned(requested, bind_source)
    }
}

impl From<&Path> for ReadablePath {
    fn from(requested: &Path) -> Self {
        requested.to_path_buf().into()
    }
}

impl From<&PathBuf> for ReadablePath {
    fn from(requested: &PathBuf) -> Self {
        requested.as_path().into()
    }
}

impl AsRef<Path> for ReadablePath {
    fn as_ref(&self) -> &Path {
        &self.requested
    }
}

impl std::ops::Deref for ReadablePath {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.requested
    }
}

impl PartialEq<PathBuf> for ReadablePath {
    fn eq(&self, other: &PathBuf) -> bool {
        self.requested == *other
    }
}

impl PartialEq<Path> for ReadablePath {
    fn eq(&self, other: &Path) -> bool {
        self.requested == other
    }
}

impl PartialEq<ReadablePath> for PathBuf {
    fn eq(&self, other: &ReadablePath) -> bool {
        *self == other.requested
    }
}

impl PartialEq<ReadablePath> for Path {
    fn eq(&self, other: &ReadablePath) -> bool {
        self == other.requested
    }
}

pub(super) fn push_runtime_daemon_socket_readable_paths(
    filesystem: FilesystemCapability,
    user_daemon_ipc: bool,
    writable_paths: &[PathBuf],
    readable_paths: &mut Vec<ReadablePath>,
) {
    if filesystem != FilesystemCapability::Bwrap || !user_daemon_ipc {
        return;
    }
    let Some(runtime_root) = runtime_path::xdg_runtime_root() else {
        return;
    };

    for socket_path in runtime_path::runtime_daemon_socket_paths(&runtime_root) {
        if !socket_path.exists()
            || path_already_exposed(readable_paths, writable_paths, &socket_path)
        {
            continue;
        }
        readable_paths.push(socket_path.into());
    }
}

fn path_already_exposed(
    readable_paths: &[ReadablePath],
    writable_paths: &[PathBuf],
    path: &Path,
) -> bool {
    readable_paths
        .iter()
        .any(|candidate| path == candidate.requested())
        || writable_paths
            .iter()
            .any(|candidate| path.starts_with(candidate))
}
