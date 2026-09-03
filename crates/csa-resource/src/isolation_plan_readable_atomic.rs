#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::io::{self, Seek, SeekFrom};
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};
#[cfg(unix)]
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(test)]
use std::cell::Cell;

#[cfg(unix)]
static ATOMIC_COPY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
thread_local! {
    pub(crate) static FAIL_ATOMIC_COPY: Cell<bool> = const { Cell::new(false) };
    pub(crate) static AFTER_ATOMIC_COPY_WRITTEN: Cell<Option<fn(&std::path::Path)>> =
        const { Cell::new(None) };
}

#[cfg(unix)]
pub(crate) fn open_pinned_regular_at(
    parent: &File,
    name: &std::ffi::OsStr,
) -> std::io::Result<Option<File>> {
    let expected = match super::stat_at(parent, name) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if expected.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "SQLite generation member is not a regular file",
        ));
    }
    let file = super::open_regular_at(parent, name)?;
    super::confirm_opened_identity(&file, &expected)?;
    Ok(Some(file))
}

#[cfg(unix)]
pub(crate) fn remove_file_at(parent: &File, name: &std::ffi::OsStr) -> std::io::Result<()> {
    let name = super::path_component(name)?;
    // SAFETY: parent is a live directory descriptor and name is one component.
    let result = unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}
#[cfg(unix)]
pub(crate) fn remove_file_if_same(
    parent: &File,
    name: &std::ffi::OsStr,
    expected: &File,
) -> std::io::Result<()> {
    let Some(current) = open_pinned_regular_at(parent, name)? else {
        return Ok(());
    };
    if !super::same_file(&current, expected)? {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "pinned temporary file changed before cleanup",
        ));
    }
    remove_file_at(parent, name)
}

#[cfg(test)]
fn run_after_atomic_copy_written(parent: &File, name: &CString) {
    AFTER_ATOMIC_COPY_WRITTEN.with(|hook| {
        if let Some(inject) = hook.get() {
            let path = std::path::PathBuf::from("/proc/self/fd")
                .join(parent.as_raw_fd().to_string())
                .join(name.to_str().unwrap());
            inject(&path);
        }
    });
}

#[cfg(unix)]
pub(crate) fn copy_pinned_file_atomic(
    source: &File,
    destination_parent: &File,
    destination_name: &std::ffi::OsStr,
) -> std::io::Result<()> {
    let destination_name = super::path_component(destination_name)?;
    let sequence = ATOMIC_COPY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary_name = CString::new(format!(
        ".csa-sqlite-copy-{}-{sequence}",
        std::process::id()
    ))
    .expect("fixed temporary SQLite name has no NUL");
    // SAFETY: destination_parent is live and the temporary name is a component.
    let fd = unsafe {
        libc::openat(
            destination_parent.as_raw_fd(),
            temporary_name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: fd is the uniquely owned descriptor returned by openat.
    let mut destination = unsafe { File::from_raw_fd(fd) };
    let result = (|| {
        #[cfg(test)]
        if FAIL_ATOMIC_COPY.with(Cell::get) {
            return Err(std::io::Error::other("injected atomic SQLite copy failure"));
        }
        let mut source = source.try_clone()?;
        source.seek(SeekFrom::Start(0))?;
        io::copy(&mut source, &mut destination)?;
        destination.sync_all()?;
        #[cfg(test)]
        run_after_atomic_copy_written(destination_parent, &temporary_name);
        let current = open_pinned_regular_at(
            destination_parent,
            std::ffi::OsStr::new(temporary_name.to_str().unwrap()),
        )?
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Interrupted,
                "temporary SQLite copy disappeared",
            )
        })?;
        if !super::same_file(&current, &destination)? {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "temporary SQLite copy changed before publication",
            ));
        }
        // SAFETY: destination_parent is a live directory descriptor.
        if unsafe { libc::fsync(destination_parent.as_raw_fd()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        #[cfg(target_os = "linux")]
        {
            // SAFETY: both descriptors are live and names are NUL-terminated.
            let result = unsafe {
                libc::syscall(
                    libc::SYS_renameat2,
                    destination_parent.as_raw_fd(),
                    temporary_name.as_ptr(),
                    destination_parent.as_raw_fd(),
                    destination_name.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            if result != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            // SAFETY: both descriptors are live and names are NUL-terminated.
            if unsafe {
                libc::renameat(
                    destination_parent.as_raw_fd(),
                    temporary_name.as_ptr(),
                    destination_parent.as_raw_fd(),
                    destination_name.as_ptr(),
                )
            } != 0
            {
                return Err(std::io::Error::last_os_error());
            }
        }
        // SAFETY: destination_parent is a live directory descriptor.
        if unsafe { libc::fsync(destination_parent.as_raw_fd()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = remove_file_if_same(
            destination_parent,
            std::ffi::OsStr::new(temporary_name.to_str().unwrap()),
            &destination,
        );
    }
    result
}
