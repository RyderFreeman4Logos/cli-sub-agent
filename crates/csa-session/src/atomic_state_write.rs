use std::ffi::{CString, OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use anyhow::{Context, anyhow};
use thiserror::Error;
use ulid::Ulid;

#[derive(Debug, Error)]
pub(crate) enum AtomicPublishError {
    #[error("publication failed before the target rename: {0:#}")]
    BeforePublish(#[source] anyhow::Error),
    #[error("target rename completed, but publication durability is unconfirmed: {0:#}")]
    PublishedButDurabilityUnconfirmed(#[source] anyhow::Error),
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AtomicWriteFault {
    BeforeRename,
    AfterRename,
    BeforeContainingDirectoryFsync,
    BeforeParentDirectoryFsync,
}

pub(crate) fn publish_bytes(
    containing_dir: &Path,
    target_path: &Path,
    contents: &[u8],
) -> Result<(), AtomicPublishError> {
    if target_path.parent() != Some(containing_dir) {
        return Err(before(anyhow!(
            "atomic publication target {} is not directly inside {}",
            target_path.display(),
            containing_dir.display()
        )));
    }
    let target_name = target_path
        .file_name()
        .ok_or_else(|| before(anyhow!("atomic publication target has no file name")))?;
    let containing = open_directory(containing_dir).map_err(before)?;
    let parent = containing_dir
        .parent()
        .map(open_directory)
        .transpose()
        .map_err(before)?;
    publish_bytes_in(
        &containing,
        parent.as_ref(),
        target_name,
        target_path,
        contents,
    )
}

pub(crate) fn publish_bytes_in(
    containing_dir: &File,
    immediate_parent: Option<&File>,
    target_name: &OsStr,
    diagnostic_path: &Path,
    contents: &[u8],
) -> Result<(), AtomicPublishError> {
    publish_bytes_in_impl(
        containing_dir,
        immediate_parent,
        target_name,
        diagnostic_path,
        contents,
        #[cfg(test)]
        None,
    )
}

#[cfg(test)]
pub(crate) fn publish_bytes_in_with_fault(
    containing_dir: &File,
    immediate_parent: Option<&File>,
    target_name: &OsStr,
    diagnostic_path: &Path,
    contents: &[u8],
    fault: AtomicWriteFault,
) -> Result<(), AtomicPublishError> {
    publish_bytes_in_impl(
        containing_dir,
        immediate_parent,
        target_name,
        diagnostic_path,
        contents,
        Some(fault),
    )
}

fn publish_bytes_in_impl(
    containing_dir: &File,
    immediate_parent: Option<&File>,
    target_name: &OsStr,
    diagnostic_path: &Path,
    contents: &[u8],
    #[cfg(test)] fault: Option<AtomicWriteFault>,
) -> Result<(), AtomicPublishError> {
    validate_name(target_name).map_err(before)?;
    let mut temp =
        create_unique_temp(containing_dir, target_name, diagnostic_path).map_err(before)?;
    set_private_mode(temp.file()).map_err(before)?;
    verify_private_regular(temp.file(), diagnostic_path).map_err(before)?;
    temp.file_mut()
        .write_all(contents)
        .with_context(|| {
            format!(
                "failed to write temporary file for {}",
                diagnostic_path.display()
            )
        })
        .map_err(before)?;
    temp.file_mut()
        .sync_all()
        .with_context(|| {
            format!(
                "failed to sync temporary file for {}",
                diagnostic_path.display()
            )
        })
        .map_err(before)?;

    #[cfg(test)]
    if fault == Some(AtomicWriteFault::BeforeRename) {
        return Err(before(anyhow!("injected failure before target rename")));
    }

    renameat(
        containing_dir.as_raw_fd(),
        temp.name(),
        containing_dir.as_raw_fd(),
        target_name,
    )
    .with_context(|| {
        format!(
            "failed to rename temporary file over {}",
            diagnostic_path.display()
        )
    })
    .map_err(before)?;
    temp.mark_published();

    #[cfg(test)]
    if fault == Some(AtomicWriteFault::AfterRename) {
        return Err(after(anyhow!(
            "injected failure immediately after target rename"
        )));
    }

    #[cfg(test)]
    if fault == Some(AtomicWriteFault::BeforeContainingDirectoryFsync) {
        return Err(after(anyhow!(
            "injected failure before containing directory fsync"
        )));
    }

    containing_dir
        .sync_all()
        .with_context(|| {
            format!(
                "failed to fsync containing directory after publishing {}",
                diagnostic_path.display()
            )
        })
        .map_err(after)?;
    if let Some(parent) = immediate_parent {
        #[cfg(test)]
        if fault == Some(AtomicWriteFault::BeforeParentDirectoryFsync) {
            return Err(after(anyhow!(
                "injected failure before parent directory fsync"
            )));
        }
        parent
            .sync_all()
            .with_context(|| {
                format!(
                    "failed to fsync immediate parent directory after publishing {}",
                    diagnostic_path.display()
                )
            })
            .map_err(after)?;
    }
    Ok(())
}

struct TemporaryPublication {
    file: File,
    directory_fd: RawFd,
    name: OsString,
    published: bool,
}

impl TemporaryPublication {
    fn file(&self) -> &File {
        &self.file
    }

    fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    fn name(&self) -> &OsStr {
        &self.name
    }

    fn mark_published(&mut self) {
        self.published = true;
    }
}

impl Drop for TemporaryPublication {
    fn drop(&mut self) {
        if self.published {
            return;
        }
        let Ok(name) = c_name(&self.name) else {
            return;
        };
        // SAFETY: `directory_fd` remains valid because the containing `File` outlives this guard;
        // `name` is NUL-terminated. Cleanup is best-effort and never follows a path.
        unsafe {
            libc::unlinkat(self.directory_fd, name.as_ptr(), 0);
        }
    }
}

fn create_unique_temp(
    containing_dir: &File,
    target_name: &OsStr,
    diagnostic_path: &Path,
) -> anyhow::Result<TemporaryPublication> {
    for _ in 0..32 {
        let name = OsString::from(format!(
            ".{}.{}.tmp",
            target_name.to_string_lossy(),
            Ulid::new()
        ));
        match openat(
            containing_dir.as_raw_fd(),
            &name,
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        ) {
            Ok(file) => {
                return Ok(TemporaryPublication {
                    file,
                    directory_fd: containing_dir.as_raw_fd(),
                    name,
                    published: false,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to create temporary publication file for {}",
                        diagnostic_path.display()
                    )
                });
            }
        }
    }
    Err(anyhow!(
        "failed to allocate a unique temporary publication name for {}",
        diagnostic_path.display()
    ))
}

fn open_directory(path: &Path) -> anyhow::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .with_context(|| format!("failed to securely open directory {}", path.display()))
}

fn validate_name(name: &OsStr) -> anyhow::Result<()> {
    let bytes = name.as_bytes();
    if bytes.is_empty()
        || bytes == b"."
        || bytes == b".."
        || bytes.contains(&b'/')
        || bytes.contains(&0)
    {
        return Err(anyhow!("invalid atomic publication target name"));
    }
    Ok(())
}

fn set_private_mode(file: &File) -> anyhow::Result<()> {
    // SAFETY: the descriptor is valid for the lifetime of `file`; every mode value is accepted.
    let result = unsafe { libc::fchmod(file.as_raw_fd(), 0o600) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error()).context("failed to set temporary file mode 0600")
    }
}

fn verify_private_regular(file: &File, diagnostic_path: &Path) -> anyhow::Result<()> {
    let metadata = file.metadata().with_context(|| {
        format!(
            "failed to inspect temporary publication file for {}",
            diagnostic_path.display()
        )
    })?;
    if !metadata.file_type().is_file() {
        return Err(anyhow!(
            "temporary publication file is not regular for {}",
            diagnostic_path.display()
        ));
    }
    // SAFETY: `geteuid` has no preconditions and does not access memory.
    let euid = unsafe { libc::geteuid() };
    if metadata.uid() != euid || metadata.mode() & 0o777 != 0o600 {
        return Err(anyhow!(
            "temporary publication file is not owned by the current user with mode 0600 for {}",
            diagnostic_path.display()
        ));
    }
    Ok(())
}

fn c_name(name: &OsStr) -> std::io::Result<CString> {
    CString::new(name.as_bytes()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "filesystem component contains NUL",
        )
    })
}

fn openat(
    directory_fd: RawFd,
    name: &OsStr,
    flags: i32,
    mode: libc::mode_t,
) -> std::io::Result<File> {
    let name = c_name(name)?;
    // SAFETY: `directory_fd` is a held directory descriptor, `name` is NUL-terminated, and the
    // returned descriptor is uniquely owned by the resulting `File`.
    let fd = unsafe { libc::openat(directory_fd, name.as_ptr(), flags, mode) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: successful `openat` returned a new uniquely owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn renameat(
    old_directory_fd: RawFd,
    old_name: &OsStr,
    new_directory_fd: RawFd,
    new_name: &OsStr,
) -> std::io::Result<()> {
    let old_name = c_name(old_name)?;
    let new_name = c_name(new_name)?;
    // SAFETY: both descriptors are held directories and both names are NUL-terminated components.
    let result = unsafe {
        libc::renameat(
            old_directory_fd,
            old_name.as_ptr(),
            new_directory_fd,
            new_name.as_ptr(),
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn before(error: anyhow::Error) -> AtomicPublishError {
    AtomicPublishError::BeforePublish(error)
}

fn after(error: anyhow::Error) -> AtomicPublishError {
    AtomicPublishError::PublishedButDurabilityUnconfirmed(error)
}
