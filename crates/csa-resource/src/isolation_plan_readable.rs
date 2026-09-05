#[cfg(unix)]
use std::ffi::CString;
#[cfg(not(unix))]
use std::fs;
#[cfg(unix)]
use std::fs::{File, OpenOptions};
use std::path::{Component, Path, PathBuf};

#[cfg(test)]
use std::cell::Cell;

#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
#[cfg(unix)]
use std::sync::Arc;

use crate::filesystem_sandbox::FilesystemCapability;
use crate::sandbox::ResourceCapability;

use super::runtime_path;

#[cfg(unix)]
#[path = "isolation_plan_readable_atomic.rs"]
mod atomic;
#[cfg(all(test, unix))]
pub(crate) use atomic::{
    AFTER_ATOMIC_COPY_IDENTITY, AFTER_ATOMIC_COPY_WRITTEN, AFTER_REMOVE_FILE_IDENTITY,
    FAIL_ATOMIC_COPY, remove_file_if_same,
};
#[cfg(unix)]
pub(crate) use atomic::{copy_pinned_file_atomic, open_pinned_regular_at};
#[cfg(unix)]
#[path = "isolation_plan_readable_recovery.rs"]
mod recovery;
#[cfg(all(test, unix))]
pub(crate) use recovery::AFTER_RESERVED_LEAF_CREATED;
#[cfg(unix)]
pub(super) use recovery::{
    create_unlinked_overlay_leaf_at, open_or_create_writable_dir_at, open_overlay_leaf_at,
    recover_reserved_names_at, reject_symlink_leaf_at,
};

/// Validated readable bind: requested destination plus the source pinned at
/// validation time so later replacement cannot change the bind (#3102).
#[derive(Debug, Clone)]
pub struct ReadablePath {
    requested: PathBuf,
    bind_source: PathBuf,
    overrides_writable_mount: bool,
    writable_bind: bool,
    #[cfg(unix)]
    source_file: Result<Option<Arc<File>>, Arc<std::io::Error>>,
}

impl PartialEq for ReadablePath {
    fn eq(&self, other: &Self) -> bool {
        self.requested == other.requested
            && self.bind_source == other.bind_source
            && self.overrides_writable_mount == other.overrides_writable_mount
            && self.writable_bind == other.writable_bind
    }
}

impl Eq for ReadablePath {}

impl ReadablePath {
    /// Requested destination stored for the sandbox mount.
    pub fn requested(&self) -> &Path {
        &self.requested
    }

    /// Bind source pinned when the path was validated or first added.
    pub fn bind_source(&self) -> &Path {
        &self.bind_source
    }

    pub(crate) fn overrides_writable_mount(&self) -> bool {
        self.overrides_writable_mount
    }

    pub(crate) fn writable_bind(&self) -> bool {
        self.writable_bind
    }

    /// Clone the file or directory descriptor pinned during overlay validation.
    #[cfg(unix)]
    pub(crate) fn pinned_source_file(&self) -> Option<Arc<File>> {
        self.source_file.as_ref().ok().cloned().flatten()
    }

    #[cfg(unix)]
    pub(crate) fn pin_error(&self) -> Option<&std::io::Error> {
        self.source_file.as_ref().err().map(Arc::as_ref)
    }

    /// Clone the descriptor held since validation for a regular-file snapshot.
    ///
    /// The descriptor is opened with component-by-component `openat` walking
    /// during validation and retained here, so later rename cannot redirect the
    /// snapshot to a different file.
    #[cfg(unix)]
    pub fn open_pinned_regular_file(&self) -> std::io::Result<File> {
        self.source_file
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .cloned()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "validated readable path is not a pinned regular file",
                )
            })?
            .try_clone()
    }

    pub(crate) fn pinned(requested: PathBuf, bind_source: PathBuf) -> Self {
        #[cfg(unix)]
        let source_file = open_pinned_source(&bind_source).map_err(Arc::new);

        Self {
            requested,
            bind_source,
            overrides_writable_mount: false,
            writable_bind: false,
            #[cfg(unix)]
            source_file,
        }
    }

    pub(crate) fn pinned_extra(requested: PathBuf, bind_source: PathBuf) -> Self {
        Self::pinned(requested, bind_source)
    }

    pub(crate) fn try_pinned(requested: PathBuf, bind_source: PathBuf) -> std::io::Result<Self> {
        #[cfg(unix)]
        let source_file = open_pinned_source(&bind_source)?;

        Ok(Self {
            requested,
            bind_source,
            overrides_writable_mount: false,
            writable_bind: false,
            #[cfg(unix)]
            source_file: Ok(source_file),
        })
    }

    #[cfg(unix)]
    pub(super) fn pinned_readonly_overlay(
        requested: PathBuf,
        bind_source: PathBuf,
        file: File,
    ) -> Self {
        Self {
            requested,
            bind_source,
            overrides_writable_mount: true,
            writable_bind: false,
            source_file: Ok(Some(Arc::new(file))),
        }
    }

    #[cfg(unix)]
    pub(super) fn pinned_writable(requested: PathBuf, file: File) -> Self {
        Self::pinned_writable_from(requested.clone(), requested, file)
    }

    #[cfg(unix)]
    pub(super) fn pinned_writable_from(
        requested: PathBuf,
        bind_source: PathBuf,
        file: File,
    ) -> Self {
        Self {
            requested,
            bind_source,
            overrides_writable_mount: false,
            writable_bind: true,
            source_file: Ok(Some(Arc::new(file))),
        }
    }

    #[cfg(test)]
    pub(super) fn try_pinned_readonly_overlay(path: PathBuf) -> std::io::Result<Self> {
        Self::try_pinned_readonly_overlay_from(path.clone(), path)
    }

    pub(super) fn try_pinned_readonly_overlay_from(
        requested: PathBuf,
        bind_source: PathBuf,
    ) -> std::io::Result<Self> {
        #[cfg(unix)]
        let source_file = open_pinned_source(&bind_source)?;
        #[cfg(not(unix))]
        if fs::symlink_metadata(&bind_source)?.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "read-only overlay cannot protect a symlink directory entry",
            ));
        }

        Ok(Self {
            requested,
            bind_source,
            overrides_writable_mount: true,
            writable_bind: false,
            #[cfg(unix)]
            source_file: Ok(source_file),
        })
    }
}

#[cfg(test)]
thread_local! {
    pub(super) static AFTER_READONLY_OVERLAY_METADATA: Cell<Option<fn(&Path)>> =
        const { Cell::new(None) };
    pub(super) static READDIR_ERROR_AFTER: Cell<Option<usize>> = const { Cell::new(None) };
}

#[cfg(test)]
fn run_after_readonly_overlay_metadata(path: &Path) {
    AFTER_READONLY_OVERLAY_METADATA.with(|hook| {
        if let Some(inject) = hook.get() {
            inject(path);
        }
    });
}

#[cfg(unix)]
fn open_pinned_source(bind_source: &Path) -> std::io::Result<Option<Arc<File>>> {
    pin_readable_source(bind_source)
}

#[cfg(unix)]
fn pin_readable_source(bind_source: &Path) -> std::io::Result<Option<Arc<File>>> {
    let mut components = bind_source.components();
    if !matches!(components.next(), Some(Component::RootDir)) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "pinned readable source must be absolute",
        ));
    }

    let components = components.collect::<Vec<_>>();
    let Some((final_component, parent_components)) = components.split_last() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "root path cannot be pinned as a readable source",
        ));
    };

    let mut parent = open_root_directory()?;
    for component in parent_components {
        let Component::Normal(name) = component else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "pinned readable source contains a non-normal path component",
            ));
        };
        parent = open_directory_at(&parent, name)?;
    }

    let Component::Normal(name) = final_component else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "pinned readable source contains a non-normal final component",
        ));
    };
    let expected = stat_at(&parent, name)?;
    #[cfg(test)]
    run_after_readonly_overlay_metadata(bind_source);

    if expected.st_mode & libc::S_IFMT == libc::S_IFLNK {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "read-only overlay cannot protect a symlink directory entry",
        ));
    }

    if expected.st_mode & libc::S_IFMT == libc::S_IFDIR {
        let file = open_directory_at(&parent, name)?;
        confirm_opened_identity(&file, &expected)?;
        return Ok(Some(Arc::new(file)));
    }

    // Non-regular paths remain path-bound for sandbox mounting. In particular,
    // never read-open FIFOs, sockets, directories, or devices just to classify
    // them.
    if expected.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Ok(None);
    }

    let file = open_regular_at(&parent, name)?;
    confirm_opened_identity(&file, &expected)?;
    Ok(Some(Arc::new(file)))
}

#[cfg(unix)]
pub(super) fn same_file(left: &File, right: &File) -> std::io::Result<bool> {
    let left = left.metadata()?;
    let right = right.metadata()?;
    Ok((left.dev(), left.ino()) == (right.dev(), right.ino()))
}

#[cfg(unix)]
fn confirm_opened_identity(file: &File, expected: &libc::stat) -> std::io::Result<()> {
    let mut actual = std::mem::MaybeUninit::<libc::stat>::zeroed();
    // SAFETY: `file` is a live descriptor and `actual` points to writable
    // storage for the kernel's stat result.
    let result = unsafe { libc::fstat(file.as_raw_fd(), actual.as_mut_ptr()) };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: fstat initialized `actual` after returning success.
    let actual = unsafe { actual.assume_init() };
    if actual.st_dev != expected.st_dev || actual.st_ino != expected.st_ino {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "pinned readable file changed while being opened",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn open_root_directory() -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open("/")
}

#[cfg(unix)]
fn path_component(name: &std::ffi::OsStr) -> std::io::Result<CString> {
    CString::new(name.as_bytes()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "pinned readable source contains a null byte",
        )
    })
}

#[cfg(unix)]
pub(super) fn open_directory_at(parent: &File, name: &std::ffi::OsStr) -> std::io::Result<File> {
    let name = path_component(name)?;
    // SAFETY: `parent` is a live directory descriptor and `name` is a valid
    // NUL-terminated path component.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY
                | libc::O_DIRECTORY
                | libc::O_NOFOLLOW
                | libc::O_CLOEXEC
                | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `fd` is the uniquely owned descriptor returned by openat.
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(unix)]
pub(super) fn stat_at(parent: &File, name: &std::ffi::OsStr) -> std::io::Result<libc::stat> {
    let name = path_component(name)?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::zeroed();
    // SAFETY: `parent` is a live directory descriptor; `name` is a valid path
    // component; `stat` points to writable storage.
    let result = unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: fstatat initialized `stat` after returning success.
    Ok(unsafe { stat.assume_init() })
}

#[cfg(unix)]
pub(super) fn open_regular_at(parent: &File, name: &std::ffi::OsStr) -> std::io::Result<File> {
    let name = path_component(name)?;
    // SAFETY: `parent` is a live directory descriptor and `name` is a valid
    // NUL-terminated path component.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `fd` is the uniquely owned descriptor returned by openat.
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(unix)]
fn errno_location() -> *mut libc::c_int {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    // SAFETY: libc exposes the calling thread's live errno slot.
    return unsafe { libc::__errno_location() };
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    // SAFETY: libc exposes the calling thread's live errno slot.
    return unsafe { libc::__error() };
}

#[cfg(unix)]
fn set_errno(value: libc::c_int) {
    // SAFETY: libc exposes the calling thread's writable errno slot.
    unsafe { *errno_location() = value };
}

#[cfg(unix)]
fn read_directory_entry(dirp: *mut libc::DIR) -> *mut libc::dirent {
    #[cfg(test)]
    if READDIR_ERROR_AFTER.with(|after| match after.get() {
        Some(0) => true,
        Some(remaining) => {
            after.set(Some(remaining - 1));
            false
        }
        None => false,
    }) {
        set_errno(libc::EIO);
        return std::ptr::null_mut();
    }
    // SAFETY: callers provide a live DIR* and consume the entry before the next call.
    unsafe { libc::readdir(dirp) }
}

#[cfg(unix)]
pub(super) fn directory_entry_names(dir: &File) -> std::io::Result<Vec<std::ffi::OsString>> {
    use std::ffi::{CStr, OsStr};
    use std::os::fd::IntoRawFd;
    use std::os::unix::ffi::OsStrExt;

    let dup = dir.try_clone()?;
    // SAFETY: dup is a live directory descriptor; SEEK_SET 0 rewinds the shared offset.
    if unsafe { libc::lseek(dup.as_raw_fd(), 0, libc::SEEK_SET) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let raw = dup.into_raw_fd();
    // SAFETY: fdopendir takes ownership of `raw`.
    let dirp = unsafe { libc::fdopendir(raw) };
    if dirp.is_null() {
        let error = std::io::Error::last_os_error();
        // SAFETY: fdopendir failed, so this function still owns `raw`.
        unsafe {
            libc::close(raw);
        }
        return Err(error);
    }
    let mut names = Vec::new();
    let result = loop {
        set_errno(0);
        let entry = read_directory_entry(dirp);
        if entry.is_null() {
            let error = std::io::Error::last_os_error();
            break if error.raw_os_error() == Some(0) {
                Ok(names)
            } else {
                Err(error)
            };
        }
        // SAFETY: readdir returned a dirent with a NUL-terminated d_name.
        let c_name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        let name = OsStr::from_bytes(c_name.to_bytes());
        if name == "." || name == ".." {
            continue;
        }
        names.push(name.to_os_string());
    };
    // SAFETY: closedir releases the DIR* and the duplicated descriptor.
    let closed = unsafe { libc::closedir(dirp) };
    if closed != 0 && result.is_ok() {
        return Err(std::io::Error::last_os_error());
    }
    result
}

impl From<PathBuf> for ReadablePath {
    fn from(requested: PathBuf) -> Self {
        let bind_source = runtime_path::canonicalize_or_fallback(&requested);
        let mut path = Self::pinned(requested.clone(), bind_source);
        #[cfg(unix)]
        if std::fs::symlink_metadata(&requested)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            path.source_file = Err(Arc::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "readable sandbox path must not be a symlink",
            )));
        }
        path
    }
}

impl From<&Path> for ReadablePath {
    fn from(requested: &Path) -> Self {
        requested.to_path_buf().into()
    }
}

impl From<&PathBuf> for ReadablePath {
    fn from(requested: &PathBuf) -> Self {
        requested.as_path().into()
    }
}

impl AsRef<Path> for ReadablePath {
    fn as_ref(&self) -> &Path {
        &self.requested
    }
}

impl std::ops::Deref for ReadablePath {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.requested
    }
}

impl PartialEq<PathBuf> for ReadablePath {
    fn eq(&self, other: &PathBuf) -> bool {
        self.requested == *other
    }
}

impl PartialEq<Path> for ReadablePath {
    fn eq(&self, other: &Path) -> bool {
        self.requested == other
    }
}

impl PartialEq<ReadablePath> for PathBuf {
    fn eq(&self, other: &ReadablePath) -> bool {
        *self == other.requested
    }
}

impl PartialEq<ReadablePath> for Path {
    fn eq(&self, other: &ReadablePath) -> bool {
        self == other.requested
    }
}

pub(super) fn push_runtime_daemon_socket_readable_paths(
    filesystem: FilesystemCapability,
    user_daemon_ipc: bool,
    writable_paths: &[PathBuf],
    readable_paths: &mut Vec<ReadablePath>,
) {
    if filesystem != FilesystemCapability::Bwrap || !user_daemon_ipc {
        return;
    }
    let Some(runtime_root) = runtime_path::xdg_runtime_root() else {
        return;
    };

    for socket_path in runtime_path::runtime_daemon_socket_paths(&runtime_root) {
        if !socket_path.exists()
            || path_already_exposed(readable_paths, writable_paths, &socket_path)
        {
            continue;
        }
        readable_paths.push(socket_path.into());
    }
}

fn path_already_exposed(
    readable_paths: &[ReadablePath],
    writable_paths: &[PathBuf],
    path: &Path,
) -> bool {
    readable_paths
        .iter()
        .any(|candidate| path == candidate.requested())
        || writable_paths
            .iter()
            .any(|candidate| path.starts_with(candidate))
}

pub(super) fn downgrade_incompatible_cgroup_filesystem(
    resource: &mut ResourceCapability,
    filesystem: FilesystemCapability,
    readable_paths: &[ReadablePath],
    degraded_reasons: &mut Vec<String>,
) {
    if *resource != ResourceCapability::CgroupV2 {
        return;
    }
    if filesystem == FilesystemCapability::Landlock {
        *resource = ResourceCapability::Setrlimit;
        degraded_reasons.push(
            "landlock cannot be combined with cgroup wrapper; falling back to setrlimit resource isolation".into(),
        );
        return;
    }
    #[cfg(unix)]
    if filesystem == FilesystemCapability::Bwrap
        && readable_paths
            .iter()
            .any(|path| path.pinned_source_file().is_some())
    {
        *resource = ResourceCapability::Setrlimit;
        degraded_reasons.push(
            "bwrap bind-fd cannot be combined with cgroup wrapper; falling back to setrlimit resource isolation".into(),
        );
    }
    #[cfg(not(unix))]
    let _ = readable_paths;
}
