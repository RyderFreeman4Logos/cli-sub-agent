use anyhow::{Context, Result};
use csa_core::audit::AuditManifest;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const DEFAULT_MANIFEST_PATH: &str = ".csa/audit/manifest.toml";

pub(crate) fn load(path: &Path) -> Result<AuditManifest> {
    if !path.exists() {
        return Ok(AuditManifest::new("."));
    }

    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read audit manifest: {}", path.display()));
    let manifest = content.and_then(|raw| {
        toml::from_str::<AuditManifest>(&raw)
            .with_context(|| format!("Failed to parse audit manifest: {}", path.display()))
    });

    match manifest {
        Ok(manifest) => Ok(manifest),
        Err(error) => recover_corrupt_manifest(path, &error),
    }
}

pub(crate) fn save(path: &Path, manifest: &AuditManifest) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("Failed to create manifest directory: {}", parent.display())
        })?;
    }

    let mut to_save = manifest.clone();
    to_save.meta.updated_at = chrono::Utc::now().to_rfc3339();

    let content =
        toml::to_string_pretty(&to_save).context("Failed to serialize audit manifest to TOML")?;
    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, content)
        .with_context(|| format!("Failed to write temporary manifest: {}", tmp_path.display()))?;
    fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "Failed to atomically replace manifest {} from {}",
            path.display(),
            tmp_path.display()
        )
    })?;
    Ok(())
}

fn recover_corrupt_manifest(path: &Path, source_error: &anyhow::Error) -> Result<AuditManifest> {
    let backup_path = corrupt_backup_path(path);
    fs::rename(path, &backup_path).with_context(|| {
        format!(
            "Failed to backup corrupt manifest {} to {}",
            path.display(),
            backup_path.display()
        )
    })?;
    tracing::warn!(
        error = %source_error,
        path = %path.display(),
        backup = %backup_path.display(),
        "Recovered corrupt audit manifest"
    );

    let minimal = AuditManifest::new(".");
    save(path, &minimal)
        .with_context(|| format!("Failed to write recovered manifest: {}", path.display()))?;
    Ok(minimal)
}

fn corrupt_backup_path(path: &Path) -> PathBuf {
    let mut backup = path.as_os_str().to_owned();
    backup.push(".corrupt");
    PathBuf::from(backup)
}
