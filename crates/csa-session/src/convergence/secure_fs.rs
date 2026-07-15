use std::ffi::{CString, OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, anyhow, bail};

const DIRECTORY_FLAGS: i32 =
    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;
const LEDGER_FLAGS: i32 = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;
const LOCK_FLAGS: i32 =
    libc::O_RDWR | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;

#[derive(Debug)]
pub(crate) struct SecureDirectory {
    directory: File,
    parent: File,
    name: OsString,
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl SecureDirectory {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn file(&self) -> &File {
        &self.directory
    }

    pub(crate) fn parent(&self) -> &File {
        &self.parent
    }

    pub(crate) fn verify_link(&self) -> anyhow::Result<()> {
        let status = status_at(self.parent.as_raw_fd(), &self.name).with_context(|| {
            format!(
                "failed to verify secure convergence directory link {}",
                self.path.display()
            )
        })?;
        if file_kind(status.st_mode) != libc::S_IFDIR
            || status.st_dev != self.device
            || status.st_ino != self.inode
        {
            bail!(
                "secure convergence directory is no longer linked at {}",
                self.path.display()
            );
        }
        verify_owner_and_directory_mode(&status, &self.path, true)
    }

    pub(crate) fn open_ledger(&self, name: &OsStr) -> anyhow::Result<Option<File>> {
        let Some(file) = open_file_at(self.directory.as_raw_fd(), name, LEDGER_FLAGS, 0)? else {
            return Ok(None);
        };
        verify_regular_file(&file, &self.path.join(name), false)?;
        Ok(Some(file))
    }

    pub(crate) fn open_lock(&self, name: &OsStr) -> anyhow::Result<File> {
        let file = open_file_at(self.directory.as_raw_fd(), name, LOCK_FLAGS, 0o600)?
            .ok_or_else(|| anyhow!("lock open unexpectedly reported a missing file"))?;
        verify_regular_owner(&file, &self.path.join(name))?;
        fchmod(&file, 0o600).with_context(|| {
            format!("failed to set secure lock mode for {}", self.path.display())
        })?;
        verify_regular_file(&file, &self.path.join(name), true)?;
        Ok(file)
    }

    pub(crate) fn open_private_subdirectory(
        &self,
        name: &OsStr,
        create: bool,
    ) -> anyhow::Result<Option<Self>> {
        let child_path = self.path.join(name);
        let Some((child, created)) = open_directory_at(&self.directory, name, &child_path, create)?
        else {
            return Ok(None);
        };
        if created {
            fchmod(&child, 0o700).with_context(|| {
                format!(
                    "failed to set secure directory mode for {}",
                    child_path.display()
                )
            })?;
        }
        let status = status_for_file(&child).with_context(|| {
            format!(
                "failed to inspect secure directory {}",
                child_path.display()
            )
        })?;
        verify_owner_and_directory_mode(&status, &child_path, true)?;
        if created {
            child.sync_all().with_context(|| {
                format!("failed to sync secure directory {}", child_path.display())
            })?;
            self.directory.sync_all().with_context(|| {
                format!(
                    "failed to sync secure directory link {}",
                    child_path.display()
                )
            })?;
        }
        Ok(Some(Self {
            directory: child,
            parent: self
                .directory
                .try_clone()
                .context("clone secure parent directory descriptor")?,
            name: name.to_os_string(),
            path: child_path,
            device: status.st_dev,
            inode: status.st_ino,
        }))
    }

    pub(crate) fn open_private_file(&self, name: &OsStr) -> anyhow::Result<Option<File>> {
        let Some(file) = open_file_at(self.directory.as_raw_fd(), name, LEDGER_FLAGS, 0)? else {
            return Ok(None);
        };
        verify_regular_file(&file, &self.path.join(name), true)?;
        Ok(Some(file))
    }

    pub(crate) fn verify_only_entry(&self, expected: &OsStr) -> anyhow::Result<()> {
        self.verify_link()?;
        let mut entries = std::fs::read_dir(&self.path)
            .with_context(|| format!("failed to list secure directory {}", self.path.display()))?;
        let entry = entries
            .next()
            .transpose()
            .with_context(|| format!("failed to read secure directory {}", self.path.display()))?
            .ok_or_else(|| {
                anyhow!(
                    "secure directory is missing expected entry {}",
                    expected.to_string_lossy()
                )
            })?;
        if entry.file_name() != expected || entries.next().is_some() {
            bail!(
                "secure directory must contain only {}: {}",
                expected.to_string_lossy(),
                self.path.display()
            );
        }
        self.verify_link()
    }

    pub(crate) fn verify_empty(&self) -> anyhow::Result<()> {
        self.verify_link()?;
        if std::fs::read_dir(&self.path)
            .with_context(|| format!("failed to list secure directory {}", self.path.display()))?
            .next()
            .is_some()
        {
            bail!("secure directory is not empty: {}", self.path.display());
        }
        self.verify_link()
    }
}

pub(crate) fn validate_absolute_normalized(path: &Path, label: &str) -> anyhow::Result<()> {
    if !path.is_absolute() {
        bail!("{label} must be absolute: {}", path.display());
    }
    let bytes = path.as_os_str().as_bytes();
    if bytes.contains(&0)
        || bytes.get(1..).is_none_or(|suffix| {
            suffix
                .split(|byte| *byte == b'/')
                .any(|component| component.is_empty() || component == b"." || component == b"..")
        })
    {
        bail!("{label} must be normalized: {}", path.display());
    }
    for component in path.components() {
        match component {
            Component::RootDir | Component::Normal(_) => {}
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                bail!("{label} must be normalized: {}", path.display());
            }
        }
    }
    Ok(())
}

pub(crate) fn open_convergence_directory(
    boundary: &Path,
    project_state_root: &Path,
    create: bool,
) -> anyhow::Result<Option<SecureDirectory>> {
    validate_absolute_normalized(boundary, "secure state boundary")?;
    validate_absolute_normalized(project_state_root, "project state root")?;
    if !project_state_root.starts_with(boundary) {
        bail!(
            "project state root {} is outside secure boundary {}",
            project_state_root.display(),
            boundary.display()
        );
    }
    let target = project_state_root.join("convergence");
    validate_absolute_normalized(&target, "convergence directory")?;

    let boundary_parent = boundary
        .parent()
        .context("secure state boundary has no external parent")?;
    let boundary_name = boundary
        .file_name()
        .context("secure state boundary has no final component")?;
    let mut current = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(boundary_parent)
        .with_context(|| {
            format!(
                "failed to open external parent {} for secure state boundary",
                boundary_parent.display()
            )
        })?;
    let mut current_path = boundary_parent.to_path_buf();
    let mut names = Vec::new();
    names.push(boundary_name.to_os_string());
    for component in target
        .strip_prefix(boundary)
        .context("convergence directory escaped secure boundary")?
        .components()
    {
        let Component::Normal(name) = component else {
            bail!("convergence path contains a non-normal component");
        };
        names.push(name.to_os_string());
    }

    let final_index = names
        .len()
        .checked_sub(1)
        .context("secure convergence path contains no components")?;
    for (index, name) in names.into_iter().enumerate() {
        let child_path = current_path.join(&name);
        let Some((child, created)) = open_directory_at(&current, &name, &child_path, create)?
        else {
            return Ok(None);
        };
        let final_component = index == final_index;
        let mut status = status_for_file(&child).with_context(|| {
            format!(
                "failed to inspect secure directory {}",
                child_path.display()
            )
        })?;
        verify_owner_and_directory_mode(&status, &child_path, false)?;
        if created || (create && final_component) {
            fchmod(&child, 0o700).with_context(|| {
                format!(
                    "failed to set secure directory mode for {}",
                    child_path.display()
                )
            })?;
            status = status_for_file(&child).with_context(|| {
                format!(
                    "failed to verify secure directory mode for {}",
                    child_path.display()
                )
            })?;
        }
        verify_owner_and_directory_mode(&status, &child_path, created || final_component)?;
        if create {
            child.sync_all().with_context(|| {
                format!("failed to sync secure directory {}", child_path.display())
            })?;
            current.sync_all().with_context(|| {
                format!(
                    "failed to sync parent link for secure directory {}",
                    child_path.display()
                )
            })?;
        }
        if final_component {
            return Ok(Some(SecureDirectory {
                directory: child,
                parent: current,
                name,
                path: child_path,
                device: status.st_dev,
                inode: status.st_ino,
            }));
        }
        current = child;
        current_path = child_path;
    }
    bail!("secure convergence path walk did not reach its target")
}

fn open_directory_at(
    parent: &File,
    name: &OsStr,
    diagnostic_path: &Path,
    create: bool,
) -> anyhow::Result<Option<(File, bool)>> {
    validate_component(name)?;
    match openat(parent.as_raw_fd(), name, DIRECTORY_FLAGS, 0) {
        Ok(file) => return Ok(Some((file, false))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match status_at(parent.as_raw_fd(), name) {
                Ok(_) => {
                    return Err(anyhow!(
                        "secure directory component is an unopenable existing entry: {}",
                        diagnostic_path.display()
                    ));
                }
                Err(status_error) if status_error.kind() == std::io::ErrorKind::NotFound => {}
                Err(status_error) => {
                    return Err(status_error).with_context(|| {
                        format!(
                            "failed to inspect missing secure component {}",
                            diagnostic_path.display()
                        )
                    });
                }
            }
            if !create {
                return Ok(None);
            }
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to securely open directory component {}",
                    diagnostic_path.display()
                )
            });
        }
    }

    let created = match mkdirat(parent.as_raw_fd(), name, 0o700) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to create secure directory {}",
                    diagnostic_path.display()
                )
            });
        }
    };
    let child = openat(parent.as_raw_fd(), name, DIRECTORY_FLAGS, 0).with_context(|| {
        format!(
            "failed to open securely created directory {}",
            diagnostic_path.display()
        )
    })?;
    Ok(Some((child, created)))
}

fn open_file_at(
    directory_fd: RawFd,
    name: &OsStr,
    flags: i32,
    mode: libc::mode_t,
) -> anyhow::Result<Option<File>> {
    validate_component(name)?;
    match openat(directory_fd, name, flags, mode) {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match status_at(directory_fd, name) {
                Ok(_) => bail!("secure file name refers to an unopenable existing entry"),
                Err(status_error) if status_error.kind() == std::io::ErrorKind::NotFound => {}
                Err(status_error) => return Err(status_error.into()),
            }
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

fn verify_regular_file(file: &File, path: &Path, exact_private_mode: bool) -> anyhow::Result<()> {
    let status = status_for_file(file)
        .with_context(|| format!("failed to inspect secure file {}", path.display()))?;
    verify_regular_owner_status(&status, path)?;
    let mode = status.st_mode & 0o777;
    if exact_private_mode {
        if mode != 0o600 {
            bail!(
                "secure file mode is not 0600 ({mode:o}): {}",
                path.display()
            );
        }
    } else if mode & 0o077 != 0 {
        bail!(
            "secure file has group/other permissions {mode:o}: {}",
            path.display()
        );
    }
    Ok(())
}

fn verify_regular_owner(file: &File, path: &Path) -> anyhow::Result<()> {
    let status = status_for_file(file)
        .with_context(|| format!("failed to inspect secure file {}", path.display()))?;
    verify_regular_owner_status(&status, path)
}

fn verify_regular_owner_status(status: &libc::stat, path: &Path) -> anyhow::Result<()> {
    if file_kind(status.st_mode) != libc::S_IFREG {
        bail!("secure file is not regular: {}", path.display());
    }
    let euid = effective_uid();
    if status.st_uid != euid {
        bail!(
            "secure file is owned by uid {}, expected {euid}: {}",
            status.st_uid,
            path.display()
        );
    }
    Ok(())
}

fn verify_owner_and_directory_mode(
    status: &libc::stat,
    path: &Path,
    exact_private_mode: bool,
) -> anyhow::Result<()> {
    if file_kind(status.st_mode) != libc::S_IFDIR {
        bail!(
            "secure path component is not a directory: {}",
            path.display()
        );
    }
    let euid = effective_uid();
    if status.st_uid != euid {
        bail!(
            "secure directory is owned by uid {}, expected {euid}: {}",
            status.st_uid,
            path.display()
        );
    }
    let mode = status.st_mode & 0o777;
    if exact_private_mode {
        if mode != 0o700 {
            bail!(
                "secure convergence directory mode is not 0700 ({mode:o}): {}",
                path.display()
            );
        }
    } else if mode & 0o022 != 0 {
        bail!(
            "secure directory is group/other writable ({mode:o}): {}",
            path.display()
        );
    }
    Ok(())
}

fn validate_component(name: &OsStr) -> anyhow::Result<()> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes == b"." || bytes == b".." || bytes.contains(&b'/') {
        bail!(
            "invalid secure filesystem component: {}",
            name.to_string_lossy()
        );
    }
    if bytes.contains(&0) {
        bail!("secure filesystem component contains NUL");
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
    // SAFETY: `directory_fd` is a held directory descriptor, `name` is NUL-terminated, and flags
    // and mode are ordinary Linux openat values. The returned descriptor is uniquely owned.
    let fd = unsafe { libc::openat(directory_fd, name.as_ptr(), flags, mode) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: successful `openat` returned a new uniquely owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn mkdirat(directory_fd: RawFd, name: &OsStr, mode: libc::mode_t) -> std::io::Result<()> {
    let name = c_name(name)?;
    // SAFETY: `directory_fd` is a held directory descriptor and `name` is NUL-terminated.
    let result = unsafe { libc::mkdirat(directory_fd, name.as_ptr(), mode) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn fchmod(file: &File, mode: libc::mode_t) -> std::io::Result<()> {
    // SAFETY: the descriptor is valid for the lifetime of `file`; every mode value is accepted.
    let result = unsafe { libc::fchmod(file.as_raw_fd(), mode) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn status_at(directory_fd: RawFd, name: &OsStr) -> std::io::Result<libc::stat> {
    let name = c_name(name)?;
    // SAFETY: zero is a valid initial byte pattern for `stat`, which `fstatat` initializes on
    // success. The descriptor and NUL-terminated name remain valid for the call.
    let mut status: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: pointers refer to initialized writable storage and a NUL-terminated component.
    let result = unsafe {
        libc::fstatat(
            directory_fd,
            name.as_ptr(),
            &mut status,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result == 0 {
        Ok(status)
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn status_for_file(file: &File) -> std::io::Result<libc::stat> {
    // SAFETY: zero is a valid initial byte pattern for `stat`, which `fstat` initializes on
    // success. The descriptor is valid for the lifetime of `file`.
    let mut status: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: the descriptor is valid and `status` points to writable storage.
    let result = unsafe { libc::fstat(file.as_raw_fd(), &mut status) };
    if result == 0 {
        Ok(status)
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn effective_uid() -> libc::uid_t {
    // SAFETY: `geteuid` has no preconditions and does not access memory.
    unsafe { libc::geteuid() }
}

fn file_kind(mode: libc::mode_t) -> libc::mode_t {
    mode & libc::S_IFMT
}
