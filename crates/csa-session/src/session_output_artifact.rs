use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path};

use anyhow::{Context, Result, bail};

pub fn publish_session_output_artifact(
    session_dir: &Path,
    file_name: &str,
    contents: &[u8],
) -> Result<()> {
    validate_file_name(file_name)?;
    let output_dir = validated_output_directory(session_dir)?;
    let target = output_dir.join(file_name);
    crate::atomic_state_write::publish_bytes(&output_dir, &target, contents)
        .map_err(|error| anyhow::anyhow!(error))
}

pub fn read_session_output_artifact(session_dir: &Path, file_name: &str) -> Result<Vec<u8>> {
    validate_file_name(file_name)?;
    let output_dir = validated_output_directory(session_dir)?;
    let path = output_dir.join(file_name);
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
        .with_context(|| format!("open private session output artifact {}", path.display()))?;
    validate_private_regular_file(&file, &path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .context("read private session output artifact")?;
    Ok(bytes)
}

fn validate_file_name(file_name: &str) -> Result<()> {
    let mut components = Path::new(file_name).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        bail!("session output artifact name must be one normal component");
    }
    Ok(())
}

fn validated_output_directory(session_dir: &Path) -> Result<std::path::PathBuf> {
    validate_owned_directory(session_dir, false)?;
    let output_dir = session_dir.join("output");
    if !output_dir.exists() {
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(&output_dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error).context("create private session output directory"),
        }
    }
    validate_owned_directory(&output_dir, false)?;
    Ok(output_dir)
}

fn validate_owned_directory(path: &Path, exact_private: bool) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("inspect session artifact directory {}", path.display()))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        bail!(
            "session artifact path is not a real directory: {}",
            path.display()
        );
    }
    let euid = effective_uid();
    if metadata.uid() != euid {
        bail!(
            "session artifact directory is owned by uid {}, expected {euid}: {}",
            metadata.uid(),
            path.display()
        );
    }
    let mode = metadata.mode() & 0o777;
    if (exact_private && mode != 0o700) || (!exact_private && mode & 0o022 != 0) {
        bail!(
            "unsafe session artifact directory mode {mode:o}: {}",
            path.display()
        );
    }
    Ok(())
}

fn validate_private_regular_file(file: &File, path: &Path) -> Result<()> {
    let metadata = file
        .metadata()
        .with_context(|| format!("inspect session output artifact {}", path.display()))?;
    let euid = effective_uid();
    if !metadata.file_type().is_file() || metadata.uid() != euid || metadata.mode() & 0o777 != 0o600
    {
        bail!("unsafe session output artifact: {}", path.display());
    }
    Ok(())
}

fn effective_uid() -> libc::uid_t {
    // SAFETY: `geteuid` has no preconditions and does not access memory.
    unsafe { libc::geteuid() }
}
