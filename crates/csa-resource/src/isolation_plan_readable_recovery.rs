#[cfg(test)]
use std::cell::Cell;
#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(all(test, unix))]
use std::path::PathBuf;

#[cfg(unix)]
pub(super) fn acquire_reserved_name_lock(parent: &File) -> std::io::Result<File> {
    // Reopen the directory so flock uses an independent file description, not
    // `parent` itself. A named lock file would remain in snapshot staging and
    // on Hermes overlays after the flock is dropped.
    let lock = super::open_directory_at(parent, std::ffi::OsStr::new("."))?;
    loop {
        // SAFETY: lock is an independent file description for `parent`. Keep
        // ownership cross-process so a live creator's name-visible window
        // serializes against recovery. ponytail: blocking per-directory flock;
        // add a deadline if wedged preflights stall later launches.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(lock);
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EINTR) {
            return Err(error);
        }
    }
}

#[cfg(test)]
thread_local! {
    pub(crate) static AFTER_RESERVED_LEAF_CREATED: Cell<Option<fn(&std::path::Path)>> =
        const { Cell::new(None) };
}

#[cfg(all(test, unix))]
pub(crate) fn run_after_reserved_leaf_created(parent: &File, name: &std::ffi::OsStr) {
    AFTER_RESERVED_LEAF_CREATED.with(|hook| {
        if let Some(inject) = hook.get() {
            let path = PathBuf::from("/proc/self/fd")
                .join(parent.as_raw_fd().to_string())
                .join(name);
            inject(&path);
        }
    });
}

#[cfg(unix)]
pub(crate) fn recover_reserved_names_at(parent: &File) -> std::io::Result<()> {
    let _lock = acquire_reserved_name_lock(parent)?;
    const PREFIXES: [&str; 3] = [".csa-sqlite-snapshot-", ".csa-absent-", ".csa-write-probe-"];
    for name in super::directory_entry_names(parent)? {
        let name_lossy = name.to_string_lossy();
        if !PREFIXES.iter().any(|prefix| name_lossy.starts_with(prefix)) {
            continue;
        }
        let c_name = super::path_component(&name)?;
        let stat = match super::stat_at(parent, &name) {
            Ok(stat) => stat,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let flags = if stat.st_mode & libc::S_IFMT == libc::S_IFDIR {
            libc::AT_REMOVEDIR
        } else {
            0
        };
        // SAFETY: parent is pinned; name is one recovered reserved component.
        let result = unsafe { libc::unlinkat(parent.as_raw_fd(), c_name.as_ptr(), flags) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(error);
            }
        }
        if flags == 0 {
            for suffix in [b"-wal".as_slice(), b"-shm", b"-journal"] {
                let mut sidecar = name.clone();
                sidecar.push(std::ffi::OsStr::from_bytes(suffix));
                let sidecar_name = super::path_component(&sidecar)?;
                // SAFETY: parent is pinned; sidecar is a reserved-name WAL/SHM/journal leaf.
                let sidecar_result =
                    unsafe { libc::unlinkat(parent.as_raw_fd(), sidecar_name.as_ptr(), 0) };
                if sidecar_result != 0 {
                    let error = std::io::Error::last_os_error();
                    if error.kind() != std::io::ErrorKind::NotFound {
                        return Err(error);
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) fn create_unlinked_overlay_leaf_at(
    parent: &File,
    name: &std::ffi::OsStr,
    directory: bool,
) -> std::io::Result<File> {
    let _lock = acquire_reserved_name_lock(parent)?;
    let name = super::path_component(name)?;
    if directory {
        // SAFETY: parent is live and name is a unique, validated component.
        if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        #[cfg(test)]
        run_after_reserved_leaf_created(parent, std::ffi::OsStr::from_bytes(name.to_bytes()));
        let file =
            match super::open_directory_at(parent, std::ffi::OsStr::from_bytes(name.to_bytes())) {
                Ok(file) => file,
                Err(error) => {
                    // SAFETY: parent is live and name identifies the placeholder created above.
                    unsafe {
                        libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR)
                    };
                    return Err(error);
                }
            };
        // SAFETY: remove the private placeholder name while retaining its open fd.
        if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        return Ok(file);
    }
    // SAFETY: parent is live; O_EXCL prevents aliasing an attacker-controlled leaf.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: fd is uniquely owned; unlink removes the alternate writable pathname.
    let file = unsafe { File::from_raw_fd(fd) };
    #[cfg(test)]
    run_after_reserved_leaf_created(parent, std::ffi::OsStr::from_bytes(name.to_bytes()));
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(file)
}

#[cfg(unix)]
pub(crate) fn open_overlay_leaf_at(parent: &File, name: &std::ffi::OsStr) -> std::io::Result<File> {
    let stat = super::stat_at(parent, name)?;
    match stat.st_mode & libc::S_IFMT {
        libc::S_IFLNK => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "read-only overlay cannot protect a symlink directory entry",
        )),
        libc::S_IFDIR => super::open_directory_at(parent, name),
        libc::S_IFREG => super::open_regular_at(parent, name),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "read-only overlay cannot protect a non-file overlay leaf",
        )),
    }
}

#[cfg(unix)]
pub(crate) fn reject_symlink_leaf_at(parent: &File, name: &std::ffi::OsStr) -> std::io::Result<()> {
    match super::stat_at(parent, name) {
        Ok(stat) if stat.st_mode & libc::S_IFMT == libc::S_IFLNK => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "runtime path is a symlink",
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
pub(crate) fn open_or_create_writable_dir_at(
    parent: &File,
    name: &std::ffi::OsStr,
) -> std::io::Result<File> {
    let c_name = super::path_component(name)?;
    // SAFETY: `parent` is a live directory descriptor and `c_name` is a valid
    // NUL-terminated path component.
    let mkdir = unsafe { libc::mkdirat(parent.as_raw_fd(), c_name.as_ptr(), 0o700) };
    if mkdir != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EEXIST) {
            return Err(error);
        }
    }
    let directory = super::open_directory_at(parent, name)?;
    use std::os::unix::fs::PermissionsExt;
    if directory.metadata()?.permissions().mode() & 0o222 == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "directory has no write permission bits",
        ));
    }
    recover_reserved_names_at(&directory)?;
    for attempt in 0..16 {
        let probe = format!(".csa-write-probe-{}-{attempt}", std::process::id());
        match create_unlinked_overlay_leaf_at(&directory, probe.as_ref(), false) {
            Ok(_) => return Ok(directory),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a unique write probe",
    ))
}
