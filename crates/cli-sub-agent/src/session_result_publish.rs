use std::fs::{self, File};
use std::io::{ErrorKind, Write};
use std::path::Path;

use anyhow::{Context, Result, anyhow};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResultFilePublishOutcome {
    Created,
    AlreadyExists,
}

pub(crate) fn publish_result_file_if_absent(
    result_path: &Path,
    contents: &str,
    file_kind: &str,
) -> Result<ResultFilePublishOutcome> {
    publish_result_file_if_absent_with_writer(result_path, contents, file_kind, |file, contents| {
        file.write_all(contents.as_bytes())?;
        file.sync_all()
    })
}

pub(crate) fn publish_result_file_if_absent_with_writer<W>(
    result_path: &Path,
    contents: &str,
    file_kind: &str,
    write_contents: W,
) -> Result<ResultFilePublishOutcome>
where
    W: FnOnce(&mut File, &str) -> std::io::Result<()>,
{
    let result_dir = result_path.parent().ok_or_else(|| {
        anyhow!(
            "Result file path has no parent for {file_kind}: {}",
            result_path.display()
        )
    })?;
    let mut temp_file = tempfile::NamedTempFile::new_in(result_dir).with_context(|| {
        format!(
            "Failed to create temporary {file_kind} in {}",
            result_dir.display()
        )
    })?;
    if let Err(err) = write_contents(temp_file.as_file_mut(), contents) {
        return Err(anyhow!(
            "Failed to write or sync {file_kind} for {}: {err}",
            result_path.display()
        ));
    }
    preserve_existing_permissions_if_present(temp_file.as_file_mut(), result_path, file_kind)?;
    match fs::hard_link(temp_file.path(), result_path) {
        Ok(()) => Ok(ResultFilePublishOutcome::Created),
        Err(err) if err.kind() == ErrorKind::AlreadyExists => {
            Ok(ResultFilePublishOutcome::AlreadyExists)
        }
        Err(err) => Err(anyhow!(
            "Failed to publish {file_kind} for {}: {err}",
            result_path.display()
        )),
    }
}

pub(crate) fn preserve_existing_permissions_if_present(
    temp_file: &mut File,
    target_path: &Path,
    file_kind: &str,
) -> Result<()> {
    let permissions = match fs::metadata(target_path) {
        Ok(metadata) => Some(metadata.permissions()),
        Err(err) if err.kind() == ErrorKind::NotFound => None,
        Err(err) => {
            return Err(err).with_context(|| {
                format!(
                    "Failed to read {file_kind} metadata before preserving permissions: {}",
                    target_path.display()
                )
            });
        }
    };
    if let Some(permissions) = permissions {
        temp_file.set_permissions(permissions).with_context(|| {
            format!(
                "Failed to preserve existing permissions for {file_kind}: {}",
                target_path.display()
            )
        })?;
    }
    Ok(())
}
