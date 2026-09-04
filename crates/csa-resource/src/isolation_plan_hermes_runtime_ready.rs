//! Exclusive-inode publication for the Hermes runtime-ready marker.

#[cfg(test)]
use std::cell::Cell;
use std::fs::File;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::PathBuf;

use super::RUNTIME_READY;

#[cfg(test)]
thread_local! {
    pub(crate) static FAIL_DIRECTORY_FSYNC: Cell<bool> = const { Cell::new(false) };
}

pub(super) fn activate_runtime_generation(
    runtime_home_fd: &File,
    database_paths: &[PathBuf],
) -> std::io::Result<()> {
    reject_non_regular_runtime_ready(runtime_home_fd)?;
    let published = std::ffi::CString::new(RUNTIME_READY).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "runtime activation marker name is invalid",
        )
    })?;
    let (mut file, staging) = create_exclusive_runtime_ready(runtime_home_fd)?;
    if let Err(error) = (|| -> std::io::Result<()> {
        for path in database_paths {
            writeln!(file, "{}", path.display())?;
        }
        file.sync_all()?;
        fsync_runtime_directory(runtime_home_fd)?;
        Ok(())
    })() {
        let _ = unsafe { libc::unlinkat(runtime_home_fd.as_raw_fd(), staging.as_ptr(), 0) };
        return Err(error);
    }
    // SAFETY: both names are single components under the held runtime directory fd.
    let renamed = unsafe {
        libc::renameat(
            runtime_home_fd.as_raw_fd(),
            staging.as_ptr(),
            runtime_home_fd.as_raw_fd(),
            published.as_ptr(),
        )
    };
    if renamed != 0 {
        let error = std::io::Error::last_os_error();
        let _ = unsafe { libc::unlinkat(runtime_home_fd.as_raw_fd(), staging.as_ptr(), 0) };
        return Err(error);
    }
    Ok(())
}

fn fsync_runtime_directory(runtime_home_fd: &File) -> std::io::Result<()> {
    #[cfg(test)]
    if FAIL_DIRECTORY_FSYNC.with(Cell::get) {
        return Err(std::io::Error::from_raw_os_error(libc::EIO));
    }
    // SAFETY: `runtime_home_fd` remains the live runtime directory descriptor.
    if unsafe { libc::fsync(runtime_home_fd.as_raw_fd()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn create_exclusive_runtime_ready(
    runtime_home_fd: &File,
) -> std::io::Result<(File, std::ffi::CString)> {
    for attempt in 0..16 {
        let staging_name = format!(".csa-runtime-ready-{}-{attempt}.tmp", std::process::id());
        let staging = std::ffi::CString::new(staging_name).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "runtime activation staging name is invalid",
            )
        })?;
        // SAFETY: `runtime_home_fd` is a live directory descriptor and `staging` is one exclusive component.
        let fd = unsafe {
            libc::openat(
                runtime_home_fd.as_raw_fd(),
                staging.as_ptr(),
                libc::O_WRONLY
                    | libc::O_CREAT
                    | libc::O_EXCL
                    | libc::O_CLOEXEC
                    | libc::O_NOFOLLOW
                    | libc::O_NONBLOCK,
                0o600,
            )
        };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                continue;
            }
            return Err(error);
        }
        // SAFETY: `fd` is the uniquely owned descriptor returned by exclusive openat.
        return Ok((unsafe { File::from_raw_fd(fd) }, staging));
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a unique runtime activation marker",
    ))
}

fn reject_non_regular_runtime_ready(runtime_home_fd: &File) -> std::io::Result<()> {
    let name = std::ffi::CString::new(RUNTIME_READY).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "runtime activation marker name is invalid",
        )
    })?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::zeroed();
    // SAFETY: `runtime_home_fd` is a live directory; `name` is one component; do not follow.
    let result = unsafe {
        libc::fstatat(
            runtime_home_fd.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        return if error.kind() == std::io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(error)
        };
    }
    // SAFETY: fstatat initialized `stat` after returning success.
    let stat = unsafe { stat.assume_init() };
    if stat.st_mode & libc::S_IFMT == libc::S_IFREG {
        return Ok(());
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "runtime activation marker is not a regular file",
    ))
}
