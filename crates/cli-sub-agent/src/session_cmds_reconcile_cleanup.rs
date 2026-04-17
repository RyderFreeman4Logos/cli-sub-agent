use anyhow::{Context, Result};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use tracing::info;

pub(crate) fn cleanup_retired_session_target_dir(session_dir: &Path) -> Result<()> {
    let target_dir = session_dir.join("target");
    let metadata = match fs::symlink_metadata(&target_dir) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(err).with_context(|| {
                format!(
                    "Failed to inspect retired session target directory {}",
                    target_dir.display()
                )
            });
        }
    };

    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(());
    }

    let size_bytes = measure_directory_size_bytes(&target_dir).with_context(|| {
        format!(
            "Failed to measure retired session target directory {}",
            target_dir.display()
        )
    })?;
    fs::remove_dir_all(&target_dir).with_context(|| {
        format!(
            "Failed to remove retired session target directory {}",
            target_dir.display()
        )
    })?;
    info!(
        "Retiring session: removed {} ({})",
        target_dir.display(),
        format_bytes(size_bytes)
    );
    Ok(())
}

fn measure_directory_size_bytes(path: &Path) -> std::io::Result<u64> {
    let mut total = 0u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();
        let metadata = fs::symlink_metadata(&entry_path)?;
        if metadata.is_dir() {
            total = total.saturating_add(measure_directory_size_bytes(&entry_path)?);
        } else if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
