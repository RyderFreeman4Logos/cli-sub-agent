use anyhow::{Context, Result};
use ignore::WalkBuilder;
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

const BINARY_SNIFF_BYTES: usize = 8 * 1024;

pub(crate) fn scan_directory(root: &Path, extra_ignores: &[String]) -> Result<Vec<PathBuf>> {
    let canonical_root = root
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize scan root: {}", root.display()))?;

    let mut builder = WalkBuilder::new(&canonical_root);
    builder.hidden(false);
    builder.git_ignore(true);
    builder.git_global(true);
    builder.git_exclude(true);
    builder.parents(true);

    let mut files = Vec::new();

    for entry in builder.build() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                tracing::debug!(error = %error, "Skipping unreadable walk entry");
                continue;
            }
        };

        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }

        let canonical_path = match entry.path().canonicalize() {
            Ok(path) => path,
            Err(error) => {
                tracing::debug!(
                    path = %entry.path().display(),
                    error = %error,
                    "Skipping file that failed canonicalization"
                );
                continue;
            }
        };
        if !canonical_path.starts_with(&canonical_root) {
            tracing::warn!(
                path = %canonical_path.display(),
                root = %canonical_root.display(),
                "Skipping file outside scan root"
            );
            continue;
        }

        let relative = canonical_path
            .strip_prefix(&canonical_root)
            .with_context(|| {
                format!(
                    "Failed to compute relative path for {} under {}",
                    canonical_path.display(),
                    canonical_root.display()
                )
            })?
            .to_path_buf();
        if relative.as_os_str().is_empty() {
            continue;
        }
        if contains_skipped_dir(&relative) || matches_extra_ignore(&relative, extra_ignores) {
            continue;
        }
        if is_binary_file(&canonical_path)? {
            continue;
        }

        files.push(relative);
    }

    files.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
    files.dedup();
    Ok(files)
}

fn is_binary_file(path: &Path) -> Result<bool> {
    let mut file = File::open(path).with_context(|| {
        format!(
            "Failed to open file during binary sniff: {}",
            path.display()
        )
    })?;
    let mut buffer = [0_u8; BINARY_SNIFF_BYTES];
    let bytes_read = file.read(&mut buffer).with_context(|| {
        format!(
            "Failed to read file during binary sniff: {}",
            path.display()
        )
    })?;
    Ok(buffer[..bytes_read].contains(&0))
}

fn contains_skipped_dir(relative_path: &Path) -> bool {
    relative_path.components().any(|component| {
        matches!(
            component,
            Component::Normal(name) if name == ".git" || name == ".csa"
        )
    })
}

fn matches_extra_ignore(relative_path: &Path, extra_ignores: &[String]) -> bool {
    extra_ignores.iter().any(|rule| {
        let trimmed = rule.trim().trim_start_matches("./").trim_end_matches('/');
        if trimmed.is_empty() {
            return false;
        }
        let ignore_path = Path::new(trimmed);
        relative_path == ignore_path || relative_path.starts_with(ignore_path)
    })
}
