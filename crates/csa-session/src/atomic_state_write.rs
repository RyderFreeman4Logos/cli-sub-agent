use anyhow::{Context, Result, anyhow};
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

pub(super) fn write_state_atomically(
    session_dir: &Path,
    state_path: &Path,
    contents: &str,
) -> Result<()> {
    let mut temp_file = tempfile::NamedTempFile::new_in(session_dir).with_context(|| {
        format!(
            "Failed to create temporary state file in {}",
            session_dir.display()
        )
    })?;
    temp_file
        .as_file_mut()
        .write_all(contents.as_bytes())
        .with_context(|| {
            format!(
                "Failed to write temporary state file: {}",
                state_path.display()
            )
        })?;
    temp_file.as_file_mut().sync_all().with_context(|| {
        format!(
            "Failed to sync temporary state file: {}",
            state_path.display()
        )
    })?;
    preserve_existing_state_permissions_if_present(temp_file.as_file_mut(), state_path)?;
    temp_file.persist(state_path).map_err(|err| {
        anyhow!(
            "Failed to persist state file {}: {}",
            state_path.display(),
            err.error
        )
    })?;

    Ok(())
}

fn preserve_existing_state_permissions_if_present(
    temp_file: &mut File,
    state_path: &Path,
) -> Result<()> {
    let permissions = match fs::metadata(state_path) {
        Ok(metadata) => Some(metadata.permissions()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => {
            return Err(err).with_context(|| {
                format!(
                    "Failed to read state file metadata before preserving permissions: {}",
                    state_path.display()
                )
            });
        }
    };
    if let Some(permissions) = permissions {
        temp_file.set_permissions(permissions).with_context(|| {
            format!(
                "Failed to preserve existing state file permissions: {}",
                state_path.display()
            )
        })?;
    }
    Ok(())
}
