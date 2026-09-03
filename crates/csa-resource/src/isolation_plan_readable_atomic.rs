#[cfg(test)]
use std::cell::Cell;
#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::io::{self, Seek, SeekFrom};
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};

#[cfg(test)]
thread_local! {
    pub(crate) static FAIL_ATOMIC_COPY: Cell<bool> = const { Cell::new(false) };
    pub(crate) static AFTER_ATOMIC_COPY_WRITTEN: Cell<Option<fn(&std::path::Path)>> =
        const { Cell::new(None) };
    pub(crate) static AFTER_ATOMIC_COPY_IDENTITY: Cell<Option<fn(&std::path::Path)>> =
        const { Cell::new(None) };
    pub(crate) static AFTER_REMOVE_FILE_IDENTITY: Cell<Option<fn(&std::path::Path)>> =
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
    #[cfg(test)]
    run_after_remove_file_identity(parent, name);
    let Some(current) = open_pinned_regular_at(parent, name)? else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "pinned temporary file changed before cleanup",
        ));
    };
    if !super::same_file(&current, expected)? {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "refusing pathname unlink after inode identity check",
        ));
    }
    remove_file_at(parent, name)
}

#[cfg(unix)]
fn remove_file_at(parent: &File, name: &std::ffi::OsStr) -> std::io::Result<()> {
    let name = super::path_component(name)?;
    // SAFETY: parent is a live directory descriptor and name is one component.
    let result = unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
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

#[cfg(test)]
fn run_after_atomic_copy_identity(parent: &File, name: &CString) {
    AFTER_ATOMIC_COPY_IDENTITY.with(|hook| {
        if let Some(inject) = hook.get() {
            let path = std::path::PathBuf::from("/proc/self/fd")
                .join(parent.as_raw_fd().to_string())
                .join(name.to_str().unwrap());
            inject(&path);
        }
    });
}

#[cfg(test)]
fn run_after_remove_file_identity(parent: &File, name: &std::ffi::OsStr) {
    AFTER_REMOVE_FILE_IDENTITY.with(|hook| {
        if let Some(inject) = hook.get() {
            let path = std::path::PathBuf::from("/proc/self/fd")
                .join(parent.as_raw_fd().to_string())
                .join(name);
            inject(&path);
        }
    });
}

#[cfg(target_os = "linux")]
pub(crate) fn create_unlinked_regular(parent: &File) -> io::Result<File> {
    let dot = CString::new(".").unwrap();
    // SAFETY: parent is a live directory descriptor; O_TMPFILE creates an unnamed inode.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            dot.as_ptr(),
            libc::O_RDWR | libc::O_TMPFILE | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd is the uniquely owned descriptor returned by openat.
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(target_os = "linux")]
fn link_unlinked_regular(
    source: &File,
    destination_parent: &File,
    destination_name: &CString,
) -> io::Result<()> {
    let src = CString::new(format!("/proc/self/fd/{}", source.as_raw_fd()))
        .expect("proc fd path has no NUL");
    // SAFETY: source is a live unnamed inode; destination_parent is a directory.
    let result = unsafe {
        libc::linkat(
            libc::AT_FDCWD,
            src.as_ptr(),
            destination_parent.as_raw_fd(),
            destination_name.as_ptr(),
            libc::AT_SYMLINK_FOLLOW,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(unix)]
pub(crate) fn copy_pinned_file_atomic(
    source: &File,
    destination_parent: &File,
    destination_name: &std::ffi::OsStr,
) -> std::io::Result<()> {
    let destination_name = super::path_component(destination_name)?;
    let mut destination = create_unlinked_regular(destination_parent)?;
    #[cfg(test)]
    if FAIL_ATOMIC_COPY.with(Cell::get) {
        return Err(std::io::Error::other("injected atomic SQLite copy failure"));
    }
    let mut source = source.try_clone()?;
    source.seek(SeekFrom::Start(0))?;
    io::copy(&mut source, &mut destination)?;
    destination.sync_all()?;
    #[cfg(test)]
    run_after_atomic_copy_written(destination_parent, &destination_name);
    #[cfg(test)]
    run_after_atomic_copy_identity(destination_parent, &destination_name);
    // SAFETY: destination_parent is a live directory descriptor.
    if unsafe { libc::fsync(destination_parent.as_raw_fd()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    link_unlinked_regular(&destination, destination_parent, &destination_name)?;
    // SAFETY: destination_parent is a live directory descriptor.
    if unsafe { libc::fsync(destination_parent.as_raw_fd()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
