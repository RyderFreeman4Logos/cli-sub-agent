//! Filesystem primitives for detached workspace lease identity and ownership files.

use std::fs::{self, Metadata, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use csa_session::convergence::{Sha256Digest, WorkspaceLeaseIdentity};

/// Trusted, same-filesystem directory that durably serializes detached workspace ownership.
#[derive(Debug, Clone)]
pub(crate) struct DetachedWorkspaceLeaseStore {
    root: PathBuf,
    device: u64,
    inode: u64,
}

impl DetachedWorkspaceLeaseStore {
    pub(crate) fn open(root: &Path) -> Result<Self> {
        let (canonical, metadata) = direct_directory_metadata("workspace lease store", root)?;
        Ok(Self {
            root: canonical,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    pub(super) fn root(&self) -> &Path {
        &self.root
    }

    pub(super) fn validate_current(&self) -> Result<Metadata> {
        let (_, metadata) = direct_directory_metadata("workspace lease store", &self.root)?;
        if metadata.dev() != self.device || metadata.ino() != self.inode {
            bail!("workspace lease store identity changed after acquisition");
        }
        Ok(metadata)
    }
}

pub(super) fn direct_directory_metadata(label: &str, path: &Path) -> Result<(PathBuf, Metadata)> {
    if !path.is_absolute() || path.as_os_str().is_empty() || path.to_str().is_none() {
        bail!(
            "{label} must be a nonempty absolute UTF-8 path: {}",
            path.display()
        );
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::CurDir | Component::ParentDir | Component::Prefix(_)
        )
    }) {
        bail!("{label} must be normalized: {}", path.display());
    }
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {label} {}", path.display()))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        bail!(
            "{label} must be a direct directory, not a symlink: {}",
            path.display()
        );
    }
    let canonical = fs::canonicalize(path)
        .with_context(|| format!("canonicalize {label} {}", path.display()))?;
    if canonical != path {
        bail!(
            "{label} must not resolve through a symlink: {} resolved to {}",
            path.display(),
            canonical.display()
        );
    }
    Ok((canonical, metadata))
}

pub(super) fn lease_file_name(identity: &WorkspaceLeaseIdentity) -> String {
    let root = identity
        .workspace_root()
        .to_str()
        .expect("validated workspace lease roots are UTF-8");
    let digest = Sha256Digest::compute(root.as_bytes());
    let digest = digest
        .as_str()
        .strip_prefix("sha256:")
        .expect("Sha256Digest serialization has a sha256 prefix");
    format!(".csa-workspace-lease-{digest}.json")
}

pub(super) fn read_lease_identity(path: &Path) -> Result<WorkspaceLeaseIdentity> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect detached workspace lease {}", path.display()))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() || metadata.nlink() != 1
    {
        bail!(
            "detached workspace lease is not a private regular file: {}",
            path.display()
        );
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .with_context(|| format!("open detached workspace lease {}", path.display()))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .context("read detached workspace lease identity")?;
    serde_json::from_slice(&bytes).context("parse detached workspace lease identity")
}
