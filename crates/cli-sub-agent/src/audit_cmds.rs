use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use csa_core::audit::{AuditManifest, AuditStatus, FileEntry};
use csa_core::types::OutputFormat;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::audit::{diff, hash, io, scan, security, topo};
use crate::cli::AuditCommands;

#[derive(Debug, Clone)]
struct StatusRow {
    path: String,
    status: AuditStatus,
    hash: String,
    auditor: Option<String>,
    /// Whether the blog file exists on disk.
    /// `Some(true)` = blog_path set and file exists,
    /// `Some(false)` = blog_path set but file missing,
    /// `None` = no blog_path configured.
    blog_exists: Option<bool>,
}

#[derive(Debug, Clone, Copy, Default)]
struct StatusSummary {
    pending: usize,
    generated: usize,
    approved: usize,
    modified: usize,
    blogs_exist: usize,
}

pub(crate) fn handle_audit(command: AuditCommands) -> Result<()> {
    match command {
        AuditCommands::Init { root, ignore } => handle_audit_init(root, ignore),
        AuditCommands::Status {
            format,
            filter,
            order,
        } => handle_audit_status(format, filter, order),
        AuditCommands::Update {
            files,
            status,
            auditor,
            blog_path,
        } => handle_audit_update(files, status, auditor, blog_path),
        AuditCommands::Approve { files, approved_by } => handle_audit_approve(files, approved_by),
        AuditCommands::Reset { files } => handle_audit_reset(files),
        AuditCommands::Sync => handle_audit_sync(),
    }
}

pub(crate) fn handle_audit_init(root: String, ignores: Vec<String>) -> Result<()> {
    let scan_root = canonical_root(Path::new(&root))?;
    let manifest_path = manifest_path(&scan_root);
    let file_hashes = scan_and_hash(&scan_root, &ignores)?;

    let mut manifest = AuditManifest::new(scan_root.display().to_string());
    manifest.meta.last_scanned_at = Some(Utc::now().to_rfc3339());

    for (path, hash_value) in file_hashes {
        manifest.files.insert(
            path,
            FileEntry {
                hash: hash_value,
                audit_status: AuditStatus::Pending,
                blog_path: None,
                auditor: None,
                approved_by: None,
                approved_at: None,
            },
        );
    }

    io::save(&manifest_path, &manifest)?;
    println!(
        "Initialized audit manifest: {} ({} files)",
        manifest_path.display(),
        manifest.files.len()
    );
    Ok(())
}

pub(crate) fn handle_audit_status(
    format: OutputFormat,
    filter: Option<String>,
    order: String,
) -> Result<()> {
    let root = current_root()?;
    let manifest_path = manifest_path(&root);
    let manifest = io::load(&manifest_path)?;
    let current_hashes = scan_and_hash(&root, &[])?;
    let manifest_diff = diff::diff_manifest(&manifest, &current_hashes);

    let modified_paths: BTreeSet<String> = manifest_diff.modified.into_iter().collect();
    let mut rows = build_status_rows(&manifest, &current_hashes, &modified_paths, &root);
    let filtered_status = filter.as_deref().map(parse_status).transpose()?;
    if let Some(expected) = filtered_status {
        rows.retain(|row| row.status == expected);
    }

    sort_rows(&mut rows, &order, &root)?;
    let summary = summarize_rows(&rows, &modified_paths);

    match format {
        OutputFormat::Text => {
            print_status_text(&rows, summary);
        }
        OutputFormat::Json => {
            print_status_json(&manifest, &manifest_path, &rows, summary);
        }
    }

    Ok(())
}

pub(crate) fn handle_audit_approve(files: Vec<String>, approved_by: String) -> Result<()> {
    let root = current_root()?;
    let path = manifest_path(&root);
    let mut manifest = io::load(&path)?;
    let approved_at = Utc::now().to_rfc3339();
    let files = expand_file_args(&files, &manifest, &root)?;
    let approved_count = files.len();

    for raw in files {
        let key = resolve_manifest_key(&raw, &root)?;
        let entry = manifest
            .files
            .get_mut(&key)
            .ok_or_else(|| anyhow!("File not found in manifest: {key}"))?;
        entry.audit_status = AuditStatus::Approved;
        entry.approved_by = Some(approved_by.clone());
        entry.approved_at = Some(approved_at.clone());
    }

    io::save(&path, &manifest)?;
    println!("Approved {} file(s).", approved_count);
    Ok(())
}

pub(crate) fn handle_audit_update(
    files: Vec<String>,
    status_str: String,
    auditor: Option<String>,
    blog_path: Option<String>,
) -> Result<()> {
    let status = parse_status(&status_str)?;
    let root = current_root()?;
    let path = manifest_path(&root);
    let mut manifest = io::load(&path)?;
    let files = expand_file_args(&files, &manifest, &root)?;
    let updated_count = files.len();

    for raw in files {
        let key = resolve_manifest_key(&raw, &root)?;
        let entry = manifest
            .files
            .get_mut(&key)
            .ok_or_else(|| anyhow!("File not found in manifest: {key}"))?;

        entry.audit_status = status;
        entry.auditor = auditor.clone();
        entry.blog_path = blog_path.clone();

        if status != AuditStatus::Approved {
            entry.approved_by = None;
            entry.approved_at = None;
        }
    }

    io::save(&path, &manifest)?;
    println!("Updated {} file(s).", updated_count);
    Ok(())
}

pub(crate) fn handle_audit_reset(files: Vec<String>) -> Result<()> {
    let root = current_root()?;
    let path = manifest_path(&root);
    let mut manifest = io::load(&path)?;
    let reset_count = files.len();

    for raw in files {
        let key = resolve_manifest_key(&raw, &root)?;
        let entry = manifest
            .files
            .get_mut(&key)
            .ok_or_else(|| anyhow!("File not found in manifest: {key}"))?;

        entry.audit_status = AuditStatus::Pending;
        entry.auditor = None;
        entry.approved_by = None;
        entry.approved_at = None;
    }

    io::save(&path, &manifest)?;
    println!("Reset {} file(s) to pending.", reset_count);
    Ok(())
}

pub(crate) fn handle_audit_sync() -> Result<()> {
    let root = current_root()?;
    let path = manifest_path(&root);
    let mut manifest = io::load(&path)?;
    let current_hashes = scan_and_hash(&root, &[])?;
    let manifest_diff = diff::diff_manifest(&manifest, &current_hashes);
    let summary = manifest_diff.summary();

    for new_path in &manifest_diff.new {
        security::validate_path(Path::new(new_path), &root)?;
        let hash_value = current_hashes
            .get(new_path)
            .cloned()
            .ok_or_else(|| anyhow!("Missing hash for new file: {new_path}"))?;
        manifest.files.insert(
            new_path.clone(),
            FileEntry {
                hash: hash_value,
                audit_status: AuditStatus::Pending,
                blog_path: None,
                auditor: None,
                approved_by: None,
                approved_at: None,
            },
        );
    }

    for modified_path in &manifest_diff.modified {
        security::validate_path(Path::new(modified_path), &root)?;
        let hash_value = current_hashes
            .get(modified_path)
            .cloned()
            .ok_or_else(|| anyhow!("Missing hash for modified file: {modified_path}"))?;
        let entry = manifest
            .files
            .get_mut(modified_path)
            .ok_or_else(|| anyhow!("Missing manifest entry for modified file: {modified_path}"))?;
        entry.hash = hash_value;
        entry.audit_status = AuditStatus::Pending;
        entry.approved_by = None;
        entry.approved_at = None;
    }

    for deleted_path in &manifest_diff.deleted {
        manifest.files.remove(deleted_path);
    }

    manifest.meta.last_scanned_at = Some(Utc::now().to_rfc3339());
    io::save(&path, &manifest)?;

    println!(
        "Sync complete: {} new, {} modified, {} deleted, {} unchanged.",
        summary.new, summary.modified, summary.deleted, summary.unchanged
    );
    Ok(())
}

fn current_root() -> Result<PathBuf> {
    canonical_root(&std::env::current_dir()?)
}

fn canonical_root(path: &Path) -> Result<PathBuf> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize root path: {}", path.display()))?;
    if !canonical.is_dir() {
        bail!("Root path is not a directory: {}", canonical.display());
    }
    Ok(canonical)
}

fn manifest_path(root: &Path) -> PathBuf {
    root.join(io::DEFAULT_MANIFEST_PATH)
}

fn scan_and_hash(root: &Path, ignores: &[String]) -> Result<BTreeMap<String, String>> {
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
fn is_glob_pattern(s: &str) -> bool {
    s.contains('*') || s.contains('?') || s.contains('[')
}

/// Expand file arguments that may contain glob patterns against manifest keys.
///
/// Arguments containing glob metacharacters are matched against the manifest's
/// file keys (relative paths). Non-glob arguments pass through unchanged.
/// Returns an error if a glob pattern matches zero files in the manifest.
fn expand_file_args(
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
            let pattern = glob::Pattern::new(arg)
                .with_context(|| format!("Invalid glob pattern: {arg}"))?;

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

fn resolve_manifest_key(raw: &str, root: &Path) -> Result<String> {
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

fn path_to_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn parse_status(value: &str) -> Result<AuditStatus> {
    match value.to_ascii_lowercase().as_str() {
        "pending" => Ok(AuditStatus::Pending),
        "generated" => Ok(AuditStatus::Generated),
        "approved" => Ok(AuditStatus::Approved),
        _ => bail!("Invalid audit status: '{value}'. Valid: pending, generated, approved"),
    }
}

fn build_status_rows(
    manifest: &AuditManifest,
    current_hashes: &BTreeMap<String, String>,
    modified_paths: &BTreeSet<String>,
    project_root: &Path,
) -> Vec<StatusRow> {
    let mut rows = Vec::with_capacity(current_hashes.len());
    for (path, current_hash) in current_hashes {
        if let Some(entry) = manifest.files.get(path) {
            // Modified files are downgraded to Pending regardless of stored status,
            // since the file content has changed since the last audit.
            let effective_status = if modified_paths.contains(path) {
                AuditStatus::Pending
            } else {
                entry.audit_status
            };
            let blog_exists = check_blog_exists(&entry.blog_path, project_root);
            rows.push(StatusRow {
                path: path.clone(),
                status: effective_status,
                hash: current_hash.clone(),
                auditor: entry.auditor.clone(),
                blog_exists,
            });
        } else {
            rows.push(StatusRow {
                path: path.clone(),
                status: AuditStatus::Pending,
                hash: current_hash.clone(),
                auditor: None,
                blog_exists: None,
            });
        }
    }
    rows
}

/// Check whether a blog file exists on disk.
///
/// Returns `Some(true)` if `blog_path` is set and the resolved file exists,
/// `Some(false)` if set but missing, or `None` if no blog path is configured.
fn check_blog_exists(blog_path: &Option<String>, project_root: &Path) -> Option<bool> {
    blog_path.as_ref().map(|bp| project_root.join(bp).exists())
}

fn summarize_rows(rows: &[StatusRow], modified_paths: &BTreeSet<String>) -> StatusSummary {
    let mut summary = StatusSummary::default();
    for row in rows {
        match row.status {
            AuditStatus::Pending => summary.pending += 1,
            AuditStatus::Generated => summary.generated += 1,
            AuditStatus::Approved => summary.approved += 1,
        }
        if modified_paths.contains(&row.path) {
            summary.modified += 1;
        }
        if row.blog_exists == Some(true) {
            summary.blogs_exist += 1;
        }
    }
    summary
}

fn sort_rows(rows: &mut [StatusRow], order: &str, project_root: &Path) -> Result<()> {
    match order {
        "topo" => {
            let paths: Vec<String> = rows.iter().map(|r| r.path.clone()).collect();
            let sorted_paths = topo::topo_sort(&paths, project_root);
            let index_map: std::collections::HashMap<&str, usize> = sorted_paths
                .iter()
                .enumerate()
                .map(|(i, p)| (p.as_str(), i))
                .collect();
            rows.sort_by(|left, right| {
                let left_idx = index_map
                    .get(left.path.as_str())
                    .copied()
                    .unwrap_or(usize::MAX);
                let right_idx = index_map
                    .get(right.path.as_str())
                    .copied()
                    .unwrap_or(usize::MAX);
                left_idx.cmp(&right_idx)
            });
            Ok(())
        }
        "depth" => {
            rows.sort_by(|left, right| {
                let left_depth = path_depth(&left.path);
                let right_depth = path_depth(&right.path);
                right_depth
                    .cmp(&left_depth)
                    .then_with(|| left.path.cmp(&right.path))
            });
            Ok(())
        }
        "alpha" => {
            rows.sort_by(|left, right| left.path.cmp(&right.path));
            Ok(())
        }
        _ => bail!("Invalid order: '{order}'. Valid: topo, depth, alpha"),
    }
}

fn path_depth(path: &str) -> usize {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .count()
}

fn print_status_text(rows: &[StatusRow], summary: StatusSummary) {
    println!(
        "{:<60} | {:<9} | {:<12} | {:<4} | AUDITOR",
        "PATH", "STATUS", "HASH", "BLOG"
    );
    println!("{}", "-".repeat(101));
    for row in rows {
        let short_hash: String = row.hash.chars().take(12).collect();
        let auditor = row.auditor.as_deref().unwrap_or("-");
        let blog_indicator = match row.blog_exists {
            Some(true) => "\u{2713}",
            Some(false) => "\u{2717}",
            None => "-",
        };
        println!(
            "{:<60} | {:<9} | {:<12} | {:<4} | {}",
            row.path, row.status, short_hash, blog_indicator, auditor
        );
    }
    println!(
        "{} pending, {} generated, {} approved, {} blogs exist ({} modified since last scan)",
        summary.pending, summary.generated, summary.approved, summary.blogs_exist, summary.modified
    );
}

fn print_status_json(
    manifest: &AuditManifest,
    manifest_path: &Path,
    rows: &[StatusRow],
    summary: StatusSummary,
) {
    let files: Vec<_> = rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "path": row.path,
                "status": row.status.to_string(),
                "hash": row.hash,
                "auditor": row.auditor,
                "blog_exists": row.blog_exists,
            })
        })
        .collect();

    let payload = serde_json::json!({
        "meta": {
            "manifest_path": manifest_path.display().to_string(),
            "project_root": manifest.meta.project_root,
            "created_at": manifest.meta.created_at,
            "updated_at": manifest.meta.updated_at,
            "last_scanned_at": manifest.meta.last_scanned_at,
        },
        "summary": {
            "pending": summary.pending,
            "generated": summary.generated,
            "approved": summary.approved,
            "modified": summary.modified,
            "blogs_exist": summary.blogs_exist,
        },
        "files": files,
    });

    println!(
        "{}",
        serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_check_blog_exists_file_present() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let blog_file = tmp.path().join("blog/post.md");
        fs::create_dir_all(blog_file.parent().unwrap()).expect("create blog dir");
        fs::write(&blog_file, "# Audit Blog").expect("write blog file");

        let blog_path = Some("blog/post.md".to_string());
        assert_eq!(check_blog_exists(&blog_path, tmp.path()), Some(true));
    }

    #[test]
    fn test_check_blog_exists_file_missing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let blog_path = Some("blog/nonexistent.md".to_string());
        assert_eq!(check_blog_exists(&blog_path, tmp.path()), Some(false));
    }

    #[test]
    fn test_check_blog_exists_no_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert_eq!(check_blog_exists(&None, tmp.path()), None);
    }

    #[test]
    fn test_build_status_rows_blog_exists() {
        let tmp = tempfile::tempdir().expect("tempdir");

        // Create an actual blog file on disk for one entry.
        let blog_file = tmp.path().join("blog/exists.md");
        fs::create_dir_all(blog_file.parent().unwrap()).expect("create blog dir");
        fs::write(&blog_file, "# Blog").expect("write blog");

        let mut manifest = AuditManifest::new(tmp.path().display().to_string());
        manifest.files.insert(
            "src/a.rs".to_string(),
            FileEntry {
                hash: "sha256:aaa".to_string(),
                audit_status: AuditStatus::Generated,
                blog_path: Some("blog/exists.md".to_string()),
                auditor: None,
                approved_by: None,
                approved_at: None,
            },
        );
        manifest.files.insert(
            "src/b.rs".to_string(),
            FileEntry {
                hash: "sha256:bbb".to_string(),
                audit_status: AuditStatus::Pending,
                blog_path: Some("blog/missing.md".to_string()),
                auditor: None,
                approved_by: None,
                approved_at: None,
            },
        );
        manifest.files.insert(
            "src/c.rs".to_string(),
            FileEntry {
                hash: "sha256:ccc".to_string(),
                audit_status: AuditStatus::Pending,
                blog_path: None,
                auditor: None,
                approved_by: None,
                approved_at: None,
            },
        );

        let mut current_hashes = BTreeMap::new();
        current_hashes.insert("src/a.rs".to_string(), "sha256:aaa".to_string());
        current_hashes.insert("src/b.rs".to_string(), "sha256:bbb".to_string());
        current_hashes.insert("src/c.rs".to_string(), "sha256:ccc".to_string());
        // A file not in manifest at all (new file).
        current_hashes.insert("src/d.rs".to_string(), "sha256:ddd".to_string());

        let modified = BTreeSet::new();
        let rows = build_status_rows(&manifest, &current_hashes, &modified, tmp.path());

        let find_row = |path: &str| rows.iter().find(|r| r.path == path).unwrap();

        assert_eq!(find_row("src/a.rs").blog_exists, Some(true));
        assert_eq!(find_row("src/b.rs").blog_exists, Some(false));
        assert_eq!(find_row("src/c.rs").blog_exists, None);
        // New file not in manifest should have blog_exists = None.
        assert_eq!(find_row("src/d.rs").blog_exists, None);
    }

    #[test]
    fn test_summary_counts_blogs_exist() {
        let rows = vec![
            StatusRow {
                path: "a.rs".to_string(),
                status: AuditStatus::Generated,
                hash: "sha256:a".to_string(),
                auditor: None,
                blog_exists: Some(true),
            },
            StatusRow {
                path: "b.rs".to_string(),
                status: AuditStatus::Pending,
                hash: "sha256:b".to_string(),
                auditor: None,
                blog_exists: Some(false),
            },
            StatusRow {
                path: "c.rs".to_string(),
                status: AuditStatus::Approved,
                hash: "sha256:c".to_string(),
                auditor: None,
                blog_exists: None,
            },
            StatusRow {
                path: "d.rs".to_string(),
                status: AuditStatus::Generated,
                hash: "sha256:d".to_string(),
                auditor: None,
                blog_exists: Some(true),
            },
        ];

        let modified = BTreeSet::new();
        let summary = summarize_rows(&rows, &modified);

        assert_eq!(summary.blogs_exist, 2);
        assert_eq!(summary.pending, 1);
        assert_eq!(summary.generated, 2);
        assert_eq!(summary.approved, 1);
    }

    #[test]
    fn test_json_output_includes_blog_exists() {
        let manifest = AuditManifest::new(".");
        let manifest_path = PathBuf::from("/tmp/test-manifest.toml");

        let rows = vec![
            StatusRow {
                path: "a.rs".to_string(),
                status: AuditStatus::Generated,
                hash: "sha256:aaa".to_string(),
                auditor: None,
                blog_exists: Some(true),
            },
            StatusRow {
                path: "b.rs".to_string(),
                status: AuditStatus::Pending,
                hash: "sha256:bbb".to_string(),
                auditor: None,
                blog_exists: Some(false),
            },
            StatusRow {
                path: "c.rs".to_string(),
                status: AuditStatus::Pending,
                hash: "sha256:ccc".to_string(),
                auditor: None,
                blog_exists: None,
            },
        ];

        let summary = StatusSummary {
            pending: 2,
            generated: 1,
            approved: 0,
            modified: 0,
            blogs_exist: 1,
        };

        // Build the JSON payload (same logic as print_status_json but capture it).
        let files: Vec<_> = rows
            .iter()
            .map(|row| {
                serde_json::json!({
                    "path": row.path,
                    "status": row.status.to_string(),
                    "hash": row.hash,
                    "auditor": row.auditor,
                    "blog_exists": row.blog_exists,
                })
            })
            .collect();

        let payload = serde_json::json!({
            "meta": {
                "manifest_path": manifest_path.display().to_string(),
                "project_root": manifest.meta.project_root,
                "created_at": manifest.meta.created_at,
                "updated_at": manifest.meta.updated_at,
                "last_scanned_at": manifest.meta.last_scanned_at,
            },
            "summary": {
                "pending": summary.pending,
                "generated": summary.generated,
                "approved": summary.approved,
                "modified": summary.modified,
                "blogs_exist": summary.blogs_exist,
            },
            "files": files,
        });

        // Verify blog_exists per file entry.
        let file_entries = payload["files"].as_array().unwrap();
        assert_eq!(file_entries[0]["blog_exists"], serde_json::json!(true));
        assert_eq!(file_entries[1]["blog_exists"], serde_json::json!(false));
        assert_eq!(file_entries[2]["blog_exists"], serde_json::json!(null));

        // Verify summary includes blogs_exist.
        assert_eq!(payload["summary"]["blogs_exist"], serde_json::json!(1));
    }

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
        let manifest = manifest_with_keys(&[
            "main.rs",
            "lib.rs",
            "src/nested.rs",
            "Cargo.toml",
        ]);
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
        let manifest = manifest_with_keys(&[
            "src/main.rs",
            "src/lib.rs",
            "Cargo.toml",
        ]);
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
}
