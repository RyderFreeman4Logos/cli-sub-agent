#[cfg(test)]
use std::cell::Cell;
#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(all(test, unix))]
use std::path::PathBuf;

#[cfg(unix)]
pub(super) const RESERVED_NAME_LOCK: &str = ".csa-reserved-name.lock";

#[cfg(unix)]
pub(super) fn acquire_reserved_name_lock(parent: &File) -> std::io::Result<File> {
    let name = CString::new(RESERVED_NAME_LOCK).expect("reserved-name lock is a valid CString");
    // SAFETY: parent is a live directory descriptor and name is one component.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: fd is the uniquely owned descriptor returned by openat.
    let lock = unsafe { File::from_raw_fd(fd) };
    loop {
        // SAFETY: lock is an independent file description. Keep ownership
        // cross-process so a live creator's name-visible window serializes
        // against recovery. ponytail: blocking per-directory flock; add a
        // deadline if wedged preflights stall later launches.
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
