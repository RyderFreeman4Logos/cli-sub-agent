#[cfg(test)]
use std::cell::Cell;
#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(all(test, unix))]
use std::path::PathBuf;

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
