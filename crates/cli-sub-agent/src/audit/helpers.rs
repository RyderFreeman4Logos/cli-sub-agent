use anyhow::{Context, Result, bail};
use csa_core::audit::{AuditManifest, AuditStatus};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::audit::{hash, io, scan, security};

pub(crate) fn current_root() -> Result<PathBuf> {
    canonical_root(&std::env::current_dir()?)
}

pub(crate) fn canonical_root(path: &Path) -> Result<PathBuf> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize root path: {}", path.display()))?;
    if !canonical.is_dir() {
        bail!("Root path is not a directory: {}", canonical.display());
    }
    Ok(canonical)
}

pub(crate) fn manifest_path(root: &Path) -> PathBuf {
    root.join(io::DEFAULT_MANIFEST_PATH)
}

pub(crate) fn scan_and_hash(root: &Path, ignores: &[String]) -> Result<BTreeMap<String, String>> {
    let mut current = BTreeMap::new();
    let files = scan::scan_directory(root, ignores)?;
    for relative in files {
        let validated = security::validate_path(&relative, root)?;
        let key = path_to_key(&relative);
        let hash_value = hash::hash_file(&validated)?;
        current.insert(key, hash_value);
    }
    Ok(current)
}

/// Returns `true` if the string contains glob metacharacters (`*`, `?`, `[`).
pub(crate) fn is_glob_pattern(s: &str) -> bool {
    s.contains('*') || s.contains('?') || s.contains('[')
}

/// Expand file arguments that may contain glob patterns against manifest keys.
///
/// Arguments containing glob metacharacters are matched against the manifest's
/// file keys (relative paths). Non-glob arguments pass through unchanged.
/// Returns an error if a glob pattern matches zero files in the manifest.
pub(crate) fn expand_file_args(
    args: &[String],
    manifest: &AuditManifest,
    _project_root: &Path,
) -> Result<Vec<String>> {
    let mut expanded = Vec::new();
    // Use literal separator so `*` does not cross `/` boundaries,
    // while `**` still matches across directories.
    let match_opts = glob::MatchOptions {
        require_literal_separator: true,
        ..Default::default()
    };

    for arg in args {
        if is_glob_pattern(arg) {
            let pattern =
                glob::Pattern::new(arg).with_context(|| format!("Invalid glob pattern: {arg}"))?;

            let matched: Vec<String> = manifest
                .files
                .keys()
                .filter(|key| pattern.matches_with(key, match_opts))
                .cloned()
                .collect();

            if matched.is_empty() {
                bail!("Glob pattern '{arg}' matched zero files in the audit manifest");
            }

            expanded.extend(matched);
        } else {
            expanded.push(arg.clone());
        }
    }

    Ok(expanded)
}

pub(crate) fn resolve_manifest_key(raw: &str, root: &Path) -> Result<String> {
    let validated = security::validate_path(Path::new(raw), root)?;
    let relative = validated.strip_prefix(root).with_context(|| {
        format!(
            "Validated path is outside root (path: {}, root: {})",
            validated.display(),
            root.display()
        )
    })?;

    if relative.as_os_str().is_empty() {
        bail!("File path resolves to root directory, expected a file: {raw}");
    }

    Ok(path_to_key(relative))
}

pub(crate) fn path_to_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(crate) fn parse_status(value: &str) -> Result<AuditStatus> {
    match value.to_ascii_lowercase().as_str() {
        "pending" => Ok(AuditStatus::Pending),
        "generated" => Ok(AuditStatus::Generated),
        "approved" => Ok(AuditStatus::Approved),
        _ => bail!("Invalid audit status: '{value}'. Valid: pending, generated, approved"),
    }
}

/// Validate that a mirror directory path is safe (relative, within project root).
///
/// Rejects absolute paths and parent traversal (`..`). The path must resolve
/// to a location within or equal to `project_root` after canonicalization.
/// If the directory does not yet exist (common for init), we validate the
/// string components without canonicalization.
pub(crate) fn validate_mirror_dir(mirror_dir: &str, project_root: &Path) -> Result<PathBuf> {
    let path = Path::new(mirror_dir);
    if path.is_absolute() {
        bail!("Mirror directory must be a relative path, got absolute: {mirror_dir}");
    }
    // Reject any component that is ".."
    for component in path.components() {
        if matches!(component, std::path::Component::ParentDir) {
            bail!("Mirror directory must not contain '..': {mirror_dir}");
        }
    }
    let resolved = project_root.join(path);
    let canonical_root = project_root.canonicalize().with_context(|| {
        format!(
            "Failed to canonicalize project root: {}",
            project_root.display()
        )
    })?;

    // Walk up from the resolved path to find the deepest existing ancestor.
    // Canonicalize it to resolve symlinks, then verify containment within
    // the project root. This catches symlink escapes even when the full
    // target path does not yet exist.
    let mut ancestor = resolved.as_path();
    while !ancestor.exists() {
        ancestor = match ancestor.parent() {
            Some(p) => p,
            None => break,
        };
    }
    if ancestor.exists() {
        let canonical_ancestor = ancestor
            .canonicalize()
            .with_context(|| format!("Failed to canonicalize ancestor: {}", ancestor.display()))?;
        if !canonical_ancestor.starts_with(&canonical_root) {
            bail!(
                "Mirror directory escapes project root: {} (ancestor {} resolved to {})",
                mirror_dir,
                ancestor.display(),
                canonical_ancestor.display()
            );
        }
    }

    if resolved.exists() {
        Ok(resolved.canonicalize().with_context(|| {
            format!(
                "Failed to canonicalize mirror directory: {}",
                resolved.display()
            )
        })?)
    } else {
        Ok(resolved)
    }
}

/// Compute the blog path by mirroring the source path under the mirror directory.
///
/// E.g., mirror_dir="./drafts", key="crates/csa-core/src/lib.rs"
///   -> "drafts/crates/csa-core/src/lib.rs.md"
///
/// When mirror_dir is ".", the blog path sits alongside the source:
///   -> "crates/csa-core/src/lib.rs.md"
pub(crate) fn compute_mirror_blog_path(mirror_dir: &str, source_key: &str) -> String {
    let mirror = Path::new(mirror_dir);
    let mirrored = mirror.join(format!("{source_key}.md"));
    // Normalize to forward slashes and strip leading "./" for consistent manifest keys.
    let normalized = mirrored.to_string_lossy().replace('\\', "/");
    normalized
        .strip_prefix("./")
        .unwrap_or(&normalized)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use csa_core::audit::FileEntry;

    /// Helper to create a manifest with a known set of file keys for glob tests.
    fn manifest_with_keys(keys: &[&str]) -> AuditManifest {
        let mut manifest = AuditManifest::new("/tmp/test-root".to_string());
        for key in keys {
            manifest.files.insert(
                key.to_string(),
                FileEntry {
                    hash: format!("sha256:{key}"),
                    audit_status: AuditStatus::Pending,
                    blog_path: None,
                    auditor: None,
                    approved_by: None,
                    approved_at: None,
                },
            );
        }
        manifest
    }

    #[test]
    fn test_expand_file_args_glob_src_double_star() {
        let manifest = manifest_with_keys(&[
            "src/main.rs",
            "src/lib.rs",
            "src/nested/deep.rs",
            "tests/integration.rs",
            "Cargo.toml",
        ]);
        let root = PathBuf::from("/tmp/test-root");
        let args = vec!["src/**".to_string()];

        let result = expand_file_args(&args, &manifest, &root).unwrap();
        assert!(result.contains(&"src/main.rs".to_string()));
        assert!(result.contains(&"src/lib.rs".to_string()));
        assert!(result.contains(&"src/nested/deep.rs".to_string()));
        assert!(!result.contains(&"tests/integration.rs".to_string()));
        assert!(!result.contains(&"Cargo.toml".to_string()));
    }

    #[test]
    fn test_expand_file_args_glob_star_rs() {
        let manifest = manifest_with_keys(&["main.rs", "lib.rs", "src/nested.rs", "Cargo.toml"]);
        let root = PathBuf::from("/tmp/test-root");
        let args = vec!["*.rs".to_string()];

        let result = expand_file_args(&args, &manifest, &root).unwrap();
        // `*.rs` should match top-level .rs files only (no path separators).
        assert!(result.contains(&"main.rs".to_string()));
        assert!(result.contains(&"lib.rs".to_string()));
        // Nested paths contain '/' so `*.rs` (without `**`) should NOT match them.
        assert!(!result.contains(&"src/nested.rs".to_string()));
    }

    #[test]
    fn test_expand_file_args_glob_zero_matches_is_error() {
        let manifest = manifest_with_keys(&["src/main.rs", "src/lib.rs"]);
        let root = PathBuf::from("/tmp/test-root");
        let args = vec!["nonexistent/**".to_string()];

        let result = expand_file_args(&args, &manifest, &root);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("matched zero files"));
    }

    #[test]
    fn test_expand_file_args_non_glob_passthrough() {
        let manifest = manifest_with_keys(&["src/main.rs"]);
        let root = PathBuf::from("/tmp/test-root");
        let args = vec!["src/main.rs".to_string(), "some/other/path.rs".to_string()];

        let result = expand_file_args(&args, &manifest, &root).unwrap();
        // Non-glob arguments pass through unchanged (not validated here).
        assert_eq!(result, vec!["src/main.rs", "some/other/path.rs"]);
    }

    #[test]
    fn test_expand_file_args_mixed_glob_and_literal() {
        let manifest = manifest_with_keys(&["src/main.rs", "src/lib.rs", "Cargo.toml"]);
        let root = PathBuf::from("/tmp/test-root");
        let args = vec!["Cargo.toml".to_string(), "src/*".to_string()];

        let result = expand_file_args(&args, &manifest, &root).unwrap();
        // Literal first, then glob-expanded entries.
        assert_eq!(result[0], "Cargo.toml");
        assert!(result.contains(&"src/main.rs".to_string()));
        assert!(result.contains(&"src/lib.rs".to_string()));
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_is_glob_pattern() {
        assert!(is_glob_pattern("src/**"));
        assert!(is_glob_pattern("*.rs"));
        assert!(is_glob_pattern("src/[ab].rs"));
        assert!(is_glob_pattern("src/??.rs"));
        assert!(!is_glob_pattern("src/main.rs"));
        assert!(!is_glob_pattern("Cargo.toml"));
    }

    #[test]
    fn test_validate_mirror_dir_rejects_absolute() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = validate_mirror_dir("/etc/evil", tmp.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("relative path"));
    }

    #[test]
    fn test_validate_mirror_dir_rejects_parent_traversal() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = validate_mirror_dir("../escape", tmp.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains(".."));
    }

    #[test]
    fn test_validate_mirror_dir_accepts_valid_relative() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = validate_mirror_dir("drafts/audit", tmp.path());
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_mirror_dir_accepts_dot() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = validate_mirror_dir(".", tmp.path());
        assert!(result.is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn test_validate_mirror_dir_rejects_symlink_escape() {
        let project = tempfile::tempdir().expect("project tempdir");
        let external = tempfile::tempdir().expect("external tempdir");

        // Create a symlink inside the project pointing outside.
        let symlink_path = project.path().join("link");
        std::os::unix::fs::symlink(external.path(), &symlink_path).expect("symlink");

        // "link/new": "link" is a symlink to external dir, "new" doesn't exist.
        // The deepest existing ancestor is "link", which resolves outside.
        let result = validate_mirror_dir("link/new", project.path());
        assert!(result.is_err(), "should reject symlink escape: {result:?}");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("escapes project root"),
            "error should mention escape"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_validate_mirror_dir_rejects_existing_symlink_dir() {
        let project = tempfile::tempdir().expect("project tempdir");
        let external = tempfile::tempdir().expect("external tempdir");

        let symlink_path = project.path().join("escape");
        std::os::unix::fs::symlink(external.path(), &symlink_path).expect("symlink");

        // "escape" exists as a symlink to an external directory.
        let result = validate_mirror_dir("escape", project.path());
        assert!(result.is_err(), "should reject symlink escape: {result:?}");
    }

    #[test]
    fn test_compute_mirror_blog_path_drafts_dir() {
        let result = compute_mirror_blog_path("./drafts", "crates/csa-core/src/lib.rs");
        assert_eq!(result, "drafts/crates/csa-core/src/lib.rs.md");
    }

    #[test]
    fn test_compute_mirror_blog_path_dot_dir() {
        // mirror_dir "." places blog alongside the source file.
        let result = compute_mirror_blog_path(".", "src/lib.rs");
        assert_eq!(result, "src/lib.rs.md");
    }

    #[test]
    fn test_compute_mirror_blog_path_nested_dir() {
        let result = compute_mirror_blog_path("output/blogs", "src/main.rs");
        assert_eq!(result, "output/blogs/src/main.rs.md");
    }
}
