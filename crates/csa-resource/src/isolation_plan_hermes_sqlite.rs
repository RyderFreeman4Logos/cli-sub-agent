//! Descriptor-pinned SQLite migration primitives for Hermes runtime state.

#[cfg(test)]
use std::cell::Cell;
#[cfg(test)]
use std::fs;
#[cfg(unix)]
use std::fs::File;
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use rusqlite::{Connection, OpenFlags, backup::Backup};

use super::readable;
use super::runtime_backing_error;

#[cfg(unix)]
static SQLITE_GENERATION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[cfg(unix)]
pub fn acquire_sqlite_generation_lock(parent: &File) -> std::io::Result<File> {
    let name = std::ffi::CString::new(".csa-sqlite-generation.lock").unwrap();
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
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    loop {
        // SAFETY: lock is a live file descriptor. Keep ownership cross-process
        // while avoiding an unbounded wait on a wedged preflight.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(lock);
        }
        let error = std::io::Error::last_os_error();
        let raw_error = error.raw_os_error();
        if raw_error != Some(libc::EAGAIN) && raw_error != Some(libc::EWOULDBLOCK) {
            return Err(error);
        }
        if std::time::Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "timed out waiting for SQLite generation lock",
            ));
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[cfg(unix)]
fn sqlite_pinned_path(file: &File) -> PathBuf {
    PathBuf::from("/proc/self/fd").join(file.as_raw_fd().to_string())
}

#[cfg(unix)]
fn sqlite_snapshot(
    source_parent: &File,
    destination_parent: &File,
    base: &std::ffi::OsStr,
    source_database: &File,
) -> anyhow::Result<(File, std::ffi::OsString)> {
    let sequence = SQLITE_GENERATION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = std::ffi::OsString::from(format!(
        ".csa-sqlite-snapshot-{}-{sequence}",
        std::process::id()
    ));
    let name_c = std::ffi::CString::new(name.as_os_str().as_bytes())
        .expect("fixed temporary SQLite name has no NUL");
    // SAFETY: destination_parent is live and the temporary name is a component.
    let fd = unsafe {
        libc::openat(
            destination_parent.as_raw_fd(),
            name_c.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: fd is the uniquely owned descriptor returned by openat.
    let snapshot_file = unsafe { File::from_raw_fd(fd) };
    let result = (|| {
        #[cfg(test)]
        if readable::FAIL_ATOMIC_COPY.with(Cell::get) {
            anyhow::bail!("injected atomic SQLite copy failure");
        }
        let source_path = sqlite_pinned_path(source_database);
        let snapshot_path = sqlite_pinned_path(&snapshot_file);
        let source = Connection::open_with_flags(
            source_path.clone(),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )?;
        #[cfg(test)]
        run_after_sqlite_source_opened(&source_path);
        let current_source = readable::open_pinned_regular_at(source_parent, base)?
            .ok_or_else(|| anyhow::anyhow!("pinned source database disappeared during snapshot"))?;
        if !readable::same_file(&current_source, source_database)? {
            anyhow::bail!("pinned source database changed during snapshot");
        }
        #[cfg(test)]
        run_after_sqlite_snapshot_created(&snapshot_path);
        let mut destination = Connection::open_with_flags(
            snapshot_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_URI,
        )?;
        let backup = Backup::new(&source, &mut destination)?;
        backup.run_to_completion(100, Duration::from_millis(1), None)?;
        drop(backup);
        drop(destination);
        drop(source);
        snapshot_file.sync_all()?;
        Ok::<(), anyhow::Error>(())
    })();
    if result.is_err() {
        // SAFETY: destination_parent is live; this name is owned by this call.
        unsafe {
            libc::unlinkat(destination_parent.as_raw_fd(), name_c.as_ptr(), 0);
        }
    }
    result.map(|()| (snapshot_file, name))
}

#[cfg(test)]
fn wait_for_sqlite_publication_barrier() -> anyhow::Result<()> {
    let Some(directory) = std::env::var_os("CSA_SQLITE_PUBLICATION_BARRIER") else {
        return Ok(());
    };
    let directory = PathBuf::from(directory);
    fs::create_dir_all(&directory)?;
    let marker = directory.join(format!("entered-{}", std::process::id()));
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(marker)?;
    let release = directory.join("release");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !release.exists() {
        if std::time::Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for SQLite publication barrier");
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

#[cfg(unix)]
pub fn migrate_sqlite_generation(
    source_parent: &File,
    destination_parent: &File,
    coordination_parent: &File,
    base: &std::ffi::OsStr,
    source_database: File,
    destination_path: &Path,
) -> anyhow::Result<()> {
    #[cfg(test)]
    wait_for_sqlite_publication_barrier()?;
    let _generation_lock = acquire_sqlite_generation_lock(coordination_parent)
        .map_err(|error| runtime_backing_error(destination_path, error))?;
    let names = [base.to_os_string()];
    let database_present = match readable::stat_at(destination_parent, &names[0]) {
        Ok(stat) if stat.st_mode & libc::S_IFMT == libc::S_IFREG => true,
        Ok(_) => anyhow::bail!("SQLite state database is not a regular file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut name = base.to_os_string();
        name.push(suffix);
        match readable::stat_at(destination_parent, &name) {
            Ok(stat) if stat.st_mode & libc::S_IFMT == libc::S_IFREG => {
                if !database_present {
                    readable::remove_file_at(destination_parent, &name)?;
                }
            }
            Ok(_) => anyhow::bail!("SQLite generation member is not a regular file"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    if database_present {
        return Ok(());
    }
    let (snapshot, snapshot_name) =
        sqlite_snapshot(source_parent, destination_parent, base, &source_database).map_err(
            |error| {
                runtime_backing_error(destination_path, std::io::Error::other(error.to_string()))
            },
        )?;
    let result = match readable::copy_pinned_file_atomic(&snapshot, destination_parent, base) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(runtime_backing_error(destination_path, error)),
    };
    // This process created the snapshot name, so only it may unlink it.
    let _ = readable::remove_file_at(destination_parent, &snapshot_name);
    result
}

#[cfg(test)]
thread_local! {
    pub(crate) static AFTER_SQLITE_SOURCE_OPENED: Cell<Option<fn(&Path)>> =
        const { Cell::new(None) };
    pub(crate) static AFTER_SQLITE_SNAPSHOT_CREATED: Cell<Option<fn(&Path)>> =
        const { Cell::new(None) };
}

#[cfg(test)]
fn run_after_sqlite_source_opened(source_path: &Path) {
    AFTER_SQLITE_SOURCE_OPENED.with(|hook| {
        if let Some(inject) = hook.get() {
            inject(source_path);
        }
    });
}

#[cfg(test)]
fn run_after_sqlite_snapshot_created(snapshot_path: &Path) {
    AFTER_SQLITE_SNAPSHOT_CREATED.with(|hook| {
        if let Some(inject) = hook.get() {
            inject(snapshot_path);
        }
    });
}
