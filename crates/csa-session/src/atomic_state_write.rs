use std::fs::{OpenOptions, Permissions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use anyhow::{Context, anyhow};
use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum AtomicPublishError {
    #[error("publication failed before the target rename: {0:#}")]
    BeforePublish(#[source] anyhow::Error),
    #[error("target rename completed, but publication durability is unconfirmed: {0:#}")]
    PublishedButDurabilityUnconfirmed(#[source] anyhow::Error),
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AtomicWriteFault {
    BeforeRename,
    AfterRename,
}

pub(crate) fn publish_bytes(
    containing_dir: &Path,
    target_path: &Path,
    contents: &[u8],
) -> Result<(), AtomicPublishError> {
    publish_bytes_impl(
        containing_dir,
        target_path,
        contents,
        #[cfg(test)]
        None,
    )
}

#[cfg(test)]
pub(crate) fn publish_bytes_with_fault(
    containing_dir: &Path,
    target_path: &Path,
    contents: &[u8],
    fault: AtomicWriteFault,
) -> Result<(), AtomicPublishError> {
    publish_bytes_impl(containing_dir, target_path, contents, Some(fault))
}

fn publish_bytes_impl(
    containing_dir: &Path,
    target_path: &Path,
    contents: &[u8],
    #[cfg(test)] fault: Option<AtomicWriteFault>,
) -> Result<(), AtomicPublishError> {
    if target_path.parent() != Some(containing_dir) {
        return Err(before(anyhow!(
            "atomic publication target {} is not directly inside {}",
            target_path.display(),
            containing_dir.display()
        )));
    }

    let mut temp_file = tempfile::NamedTempFile::new_in(containing_dir)
        .with_context(|| {
            format!(
                "failed to create temporary publication file in {}",
                containing_dir.display()
            )
        })
        .map_err(before)?;
    temp_file
        .as_file()
        .set_permissions(Permissions::from_mode(0o600))
        .with_context(|| {
            format!(
                "failed to set temporary publication file mode for {}",
                target_path.display()
            )
        })
        .map_err(before)?;
    temp_file
        .as_file_mut()
        .write_all(contents)
        .with_context(|| {
            format!(
                "failed to write temporary file for {}",
                target_path.display()
            )
        })
        .map_err(before)?;
    temp_file
        .as_file_mut()
        .sync_all()
        .with_context(|| {
            format!(
                "failed to sync temporary file for {}",
                target_path.display()
            )
        })
        .map_err(before)?;

    #[cfg(test)]
    if fault == Some(AtomicWriteFault::BeforeRename) {
        return Err(before(anyhow!("injected failure before target rename")));
    }

    temp_file.persist(target_path).map_err(|error| {
        before(anyhow!(
            "failed to rename temporary file over {}: {}",
            target_path.display(),
            error.error
        ))
    })?;

    #[cfg(test)]
    if fault == Some(AtomicWriteFault::AfterRename) {
        return Err(after(anyhow!(
            "injected failure immediately after target rename"
        )));
    }

    sync_directory(containing_dir)
        .with_context(|| {
            format!(
                "failed to fsync containing directory after publishing {}",
                target_path.display()
            )
        })
        .map_err(after)?;
    if let Some(parent) = containing_dir.parent() {
        sync_directory(parent)
            .with_context(|| {
                format!(
                    "failed to fsync immediate parent directory after publishing {}",
                    target_path.display()
                )
            })
            .map_err(after)?;
    }

    Ok(())
}

fn sync_directory(path: &Path) -> anyhow::Result<()> {
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
        .with_context(|| format!("failed to open directory {}", path.display()))?;
    directory
        .sync_all()
        .with_context(|| format!("failed to sync directory {}", path.display()))
}

fn before(error: anyhow::Error) -> AtomicPublishError {
    AtomicPublishError::BeforePublish(error)
}

fn after(error: anyhow::Error) -> AtomicPublishError {
    AtomicPublishError::PublishedButDurabilityUnconfirmed(error)
}
