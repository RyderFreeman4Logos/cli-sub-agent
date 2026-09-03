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
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use rusqlite::{
    Connection, OpenFlags,
    backup::{Backup, StepResult},
};

use super::readable;
use super::runtime_backing_error;

#[cfg(unix)]
const SQLITE_STAGING_DIR: &str = ".csa-sqlite-staging";

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
    staging_root: &File,
    base: &std::ffi::OsStr,
    source_database: &File,
) -> anyhow::Result<File> {
    let staging = readable::open_or_create_writable_dir_at(
        staging_root,
        std::ffi::OsStr::new(SQLITE_STAGING_DIR),
    )?;
    let current_directory = b".\0";
    // SAFETY: staging is live and O_TMPFILE creates an unnamed inode in it.
    let fd = unsafe {
        libc::openat(
            staging.as_raw_fd(),
            current_directory.as_ptr().cast(),
            libc::O_RDWR | libc::O_TMPFILE | libc::O_CLOEXEC,
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
        let source = Connection::open_with_flags(
            source_path.clone(),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )?;
        source.busy_timeout(Duration::ZERO)?;
        #[cfg(test)]
        run_after_sqlite_source_opened(&source_path);
        let current_source = readable::open_pinned_regular_at(source_parent, base)?
            .ok_or_else(|| anyhow::anyhow!("pinned source database disappeared during snapshot"))?;
        if !readable::same_file(&current_source, source_database)? {
            anyhow::bail!("pinned source database changed during snapshot");
        }
        let mut destination = Connection::open_in_memory()?;
        destination.busy_timeout(Duration::ZERO)?;
        destination.execute_batch("PRAGMA journal_mode=OFF;")?;
        let backup = Backup::new(&source, &mut destination)?;
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            if std::time::Instant::now() >= deadline {
                anyhow::bail!("timed out while snapshotting SQLite generation");
            }
            match backup.step(100)? {
                StepResult::Done => break,
                StepResult::More => {}
                StepResult::Busy | StepResult::Locked => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                _ => anyhow::bail!("unexpected SQLite backup step result"),
            }
        }
        drop(backup);
        let snapshot = destination.serialize("main")?;
        {
            use std::io::Write;
            let mut output = snapshot_file.try_clone()?;
            output.write_all(&snapshot)?;
            output.sync_all()?;
        }
        drop(snapshot);
        drop(destination);
        drop(source);
        snapshot_file.sync_all()?;
        #[cfg(test)]
        run_after_sqlite_snapshot_created(&sqlite_pinned_path(&snapshot_file));
        Ok::<(), anyhow::Error>(())
    })();
    result.map(|()| snapshot_file)
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
    if let Some(database) = readable::open_pinned_regular_at(destination_parent, base)? {
        if sqlite_generation_is_usable(&database)? {
            return Ok(());
        }
        anyhow::bail!("SQLite destination is not a usable SQLite generation");
    }
    #[cfg(test)]
    run_after_sqlite_destination_observed(destination_parent, base);
    if destination_has_usable_sqlite(destination_parent, base)? {
        return Ok(());
    }
    for suffix in ["-wal", "-shm", "-journal"] {
        if destination_has_usable_sqlite(destination_parent, base)? {
            return Ok(());
        }
        let mut name = base.to_os_string();
        name.push(suffix);
        if readable::open_pinned_regular_at(destination_parent, &name)?.is_some() {
            anyhow::bail!("incomplete SQLite generation");
        }
    }
    if destination_has_usable_sqlite(destination_parent, base)? {
        return Ok(());
    }
    let snapshot = sqlite_snapshot(source_parent, coordination_parent, base, &source_database)
        .map_err(|error| {
            runtime_backing_error(destination_path, std::io::Error::other(error.to_string()))
        })?;
    match readable::copy_pinned_file_atomic(&snapshot, destination_parent, base) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            validate_usable_sqlite_generation(destination_parent, base)
        }
        Err(error) => Err(runtime_backing_error(destination_path, error)),
    }
}

#[cfg(unix)]
fn destination_has_usable_sqlite(
    destination_parent: &File,
    base: &std::ffi::OsStr,
) -> anyhow::Result<bool> {
    let Some(database) = readable::open_pinned_regular_at(destination_parent, base)? else {
        return Ok(false);
    };
    if sqlite_generation_is_usable(&database)? {
        return Ok(true);
    }
    anyhow::bail!("SQLite destination is not a usable SQLite generation");
}

#[cfg(unix)]
fn sqlite_header_is_valid(file: &File) -> std::io::Result<bool> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(0))?;
    let mut header = [0u8; 16];
    let n = file.read(&mut header)?;
    Ok(n == 16 && header == *b"SQLite format 3\0")
}

#[cfg(unix)]
fn sqlite_generation_is_usable(file: &File) -> std::io::Result<bool> {
    if !sqlite_header_is_valid(file)? {
        return Ok(false);
    }
    let path = sqlite_pinned_path(file);
    let Ok(connection) = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    ) else {
        return Ok(false);
    };
    if connection.busy_timeout(Duration::ZERO).is_err() {
        return Ok(false);
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    connection.progress_handler(1_000, Some(move || std::time::Instant::now() >= deadline));
    match connection.query_row("PRAGMA quick_check(1)", [], |row| row.get::<_, String>(0)) {
        Ok(result) => Ok(result == "ok"),
        Err(_) => Ok(false),
    }
}

#[cfg(unix)]
fn validate_usable_sqlite_generation(
    destination_parent: &File,
    base: &std::ffi::OsStr,
) -> anyhow::Result<()> {
    let Some(database) = readable::open_pinned_regular_at(destination_parent, base)? else {
        anyhow::bail!("SQLite publication winner disappeared or is not regular");
    };
    if !sqlite_generation_is_usable(&database)? {
        anyhow::bail!("SQLite publication winner is not a usable SQLite generation");
    }
    Ok(())
}
#[cfg(test)]
thread_local! {
    pub(crate) static AFTER_SQLITE_SOURCE_OPENED: Cell<Option<fn(&Path)>> =
        const { Cell::new(None) };
    pub(crate) static AFTER_SQLITE_SNAPSHOT_CREATED: Cell<Option<fn(&Path)>> =
        const { Cell::new(None) };
    pub(crate) static AFTER_SQLITE_DESTINATION_OBSERVED: Cell<Option<fn(&Path)>> =
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

#[cfg(test)]
fn run_after_sqlite_destination_observed(parent: &File, name: &std::ffi::OsStr) {
    AFTER_SQLITE_DESTINATION_OBSERVED.with(|hook| {
        if let Some(inject) = hook.get() {
            let path = PathBuf::from("/proc/self/fd")
                .join(parent.as_raw_fd().to_string())
                .join(name);
            inject(&path);
        }
    });
}
