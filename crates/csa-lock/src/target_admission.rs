use anyhow::{Context, Result};
use std::ffi::CString;
use std::fs::{self, File};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::path::{Path, PathBuf};

const MIRROR_ROOT: &str = "/ssd/mirror-rootfs";

/// Shared parent-directory admission held while a Rust session may use Cargo's
/// canonical target. The external GC wrapper owns the marker and its exclusive
/// lock; this guard only owns a shared flock on the same parent directory inode.
///
/// The lease fd is intentionally **not** close-on-exec. Session descendants must
/// inherit the shared flock across `fork`/`exec` so process exit of an ancestor
/// cannot free the parent-inode admission while unreaped descendants still run.
pub struct TargetGcAdmissionLease {
    pub(crate) file: File,
    parent: PathBuf,
}

impl std::fmt::Debug for TargetGcAdmissionLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TargetGcAdmissionLease")
            .field("parent", &self.parent)
            .finish()
    }
}

impl Drop for TargetGcAdmissionLease {
    fn drop(&mut self) {
        // SAFETY: `file` owns the fd that holds this advisory lock.
        unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

/// Acquire a shared, non-blocking target-GC admission lease when the canonical
/// parent for `workspace` exists.
///
/// A present canonical parent is sufficient: a wrapper may expose the managed
/// symlink after this check, so dropping a successful shared lock based on the
/// mutable workspace `target` entry would race that opt-in. The workspace link
/// is therefore only consulted when the parent is absent, to fail closed for an
/// exact dangling managed link. Unmanaged workspaces whose parent is absent
/// return `Ok(None)`.
///
/// This is a cooperating advisory-lock protocol. All managed GC, fix-target,
/// and launcher actors must flock this same parent before mutating its
/// parent/target topology. A raw rename that bypasses that protocol is hostile
/// replacement and is not protected by `flock`.
pub fn acquire_target_gc_admission(workspace: &Path) -> Result<Option<TargetGcAdmissionLease>> {
    acquire_target_gc_admission_at_root_after_lock(workspace, Path::new(MIRROR_ROOT), || {})
}

#[cfg(test)]
pub(crate) fn acquire_target_gc_admission_for_test(
    workspace: &Path,
    mirror_root: &Path,
    after_lock: impl FnOnce(),
) -> Result<Option<TargetGcAdmissionLease>> {
    acquire_target_gc_admission_at_root_after_lock(workspace, mirror_root, after_lock)
}

fn acquire_target_gc_admission_at_root_after_lock(
    workspace: &Path,
    mirror_root: &Path,
    after_lock: impl FnOnce(),
) -> Result<Option<TargetGcAdmissionLease>> {
    let parent = expected_target_gc_parent_at_root(workspace, mirror_root)?;
    let expected_target = parent.join("target");
    let file = match open_directory_cloexec(&parent) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // The parent is absent, so only a lexical target probe can distinguish
            // an unmanaged workspace from a dangling managed target. Never create it.
            if has_expected_target_symlink(workspace, &expected_target)? {
                return Err(error).with_context(|| {
                    format!(
                        "target GC admission failed to open canonical target parent '{}'",
                        parent.display()
                    )
                });
            }
            return Ok(None);
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "target GC admission failed to open canonical target parent '{}'",
                    parent.display()
                )
            });
        }
    };
    let fd = file.as_raw_fd();
    // SAFETY: `file` owns a valid directory fd. The result is handled below.
    if unsafe { libc::flock(fd, libc::LOCK_SH | libc::LOCK_NB) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            anyhow::bail!(
                "target GC admission busy: canonical target parent '{}' is exclusively locked by target GC; retry after GC completes",
                parent.display()
            );
        }
        return Err(error).with_context(|| {
            format!(
                "target GC admission failed to lock canonical target parent '{}'",
                parent.display()
            )
        });
    }
    // Open used O_CLOEXEC for safety during setup; clear it so session
    // descendants inherit the shared admission flock across exec.
    clear_fd_cloexec(fd, &parent)?;

    after_lock();
    let mut fd_stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `fd_stat` points to writable storage and `fd` remains owned by `file`.
    if unsafe { libc::fstat(fd, fd_stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error()).with_context(|| {
            format!(
                "failed to fstat canonical target parent '{}'",
                parent.display()
            )
        });
    }
    // SAFETY: fstat succeeded and initialized the structure.
    let fd_stat = unsafe { fd_stat.assume_init() };
    let path_stat = fs::metadata(&parent).with_context(|| {
        format!(
            "failed to stat canonical target parent '{}'",
            parent.display()
        )
    })?;
    if fd_stat.st_dev != path_stat.dev() || fd_stat.st_ino != path_stat.ino() {
        anyhow::bail!(
            "target GC admission failed closed: canonical target parent '{}' identity changed during admission",
            parent.display()
        );
    }
    Ok(Some(TargetGcAdmissionLease { file, parent }))
}

fn expected_target_gc_parent_at_root(workspace: &Path, mirror_root: &Path) -> Result<PathBuf> {
    if !workspace.is_absolute() {
        anyhow::bail!(
            "target GC admission requires an absolute workspace path, got '{}'",
            workspace.display()
        );
    }
    let relative = workspace
        .strip_prefix("/")
        .expect("absolute workspace has a root prefix");
    Ok(mirror_root.join(relative))
}

fn has_expected_target_symlink(workspace: &Path, expected_target: &Path) -> Result<bool> {
    match fs::read_link(workspace.join("target")) {
        Ok(destination) => Ok(destination == expected_target),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(error).with_context(|| {
            format!(
                "target GC admission failed to inspect workspace target symlink '{}'",
                workspace.join("target").display()
            )
        }),
    }
}

fn open_directory_cloexec(parent: &Path) -> std::io::Result<File> {
    let path = CString::new(parent.as_os_str().as_bytes())
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    // SAFETY: `path` is NUL terminated and lives through the open call.
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd == -1 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `open` succeeded, so this function transfers ownership of `fd` to File.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn clear_fd_cloexec(fd: RawFd, path: &Path) -> Result<()> {
    // SAFETY: `fd` is owned by a live `File`; F_GETFD reads descriptor flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags == -1 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("Failed to read fd flags for {}", path.display()));
    }

    // SAFETY: `fd` is valid; F_SETFD updates only close-on-exec flags.
    let ret = unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) };
    if ret == -1 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("Failed to clear FD_CLOEXEC on {}", path.display()));
    }

    Ok(())
}
