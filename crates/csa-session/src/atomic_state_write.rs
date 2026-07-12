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

    // fsync the session directory so the rename is durable on disk.
    // Without this, a crash immediately after the rename may leave the
    // state.toml invisible to readers, causing session-registry loss
    // between SESSION_STARTED and the first `csa session wait` (#2648).
    //
    // We propagate the session-directory fsync error because it directly
    // proves the rename durability. Ancestor directory fsync is best-effort
    // because some legitimate filesystem layouts (e.g. execute-only ancestors,
    // network filesystems) may not support directory fsync.
    //
    // Directory fsync is only meaningful on POSIX (Linux/macOS). On Windows
    // the rename is already atomic within the same volume.
    #[cfg(unix)]
    {
        let dir = std::fs::File::open(session_dir).with_context(|| {
            format!(
                "Failed to open session dir for fsync: {}",
                session_dir.display()
            )
        })?;
        dir.sync_all().with_context(|| {
            format!(
                "Failed to fsync session dir after state rename: {}",
                session_dir.display()
            )
        })?;

        // Sync the immediate parent directory to persist the directory entry
        // linking this session. We propagate this error because the parent
        // (sessions/) was just created/readable via create_dir_all, so fsync
        // failure indicates a real I/O problem (#2648).
        if let Some(parent_dir) = session_dir.parent() {
            std::fs::File::open(parent_dir)
                .and_then(|f| f.sync_all())
                .with_context(|| format!("Failed to fsync parent dir: {}", parent_dir.display()))?;
        }
    }

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
