use anyhow::{Context, Result, bail};
use std::path::{Component, Path, PathBuf};

pub(crate) fn validate_path(path: &Path, root: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        bail!("Absolute paths are not allowed: {}", path.display());
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        bail!("Parent traversal is not allowed: {}", path.display());
    }

    let canonical_root = root
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize root path: {}", root.display()))?;
    let candidate = canonical_root.join(path);
    let canonical_path = candidate
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize path: {}", candidate.display()))?;

    if !canonical_path.starts_with(&canonical_root) {
        bail!(
            "Path escapes root via symlink or traversal: {} (root: {})",
            canonical_path.display(),
            canonical_root.display()
        );
    }

    Ok(canonical_path)
}
