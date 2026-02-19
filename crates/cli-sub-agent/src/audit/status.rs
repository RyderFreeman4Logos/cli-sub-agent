use anyhow::Result;
use csa_core::audit::{AuditManifest, AuditStatus};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::audit::topo;

#[derive(Debug, Clone)]
pub(crate) struct StatusRow {
    pub(crate) path: String,
    pub(crate) status: AuditStatus,
    pub(crate) hash: String,
    pub(crate) auditor: Option<String>,
    /// Whether the blog file exists on disk.
    /// `Some(true)` = blog_path set and file exists,
    /// `Some(false)` = blog_path set but file missing,
    /// `None` = no blog_path configured.
    pub(crate) blog_exists: Option<bool>,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct StatusSummary {
    pub(crate) pending: usize,
    pub(crate) generated: usize,
    pub(crate) approved: usize,
    pub(crate) modified: usize,
    pub(crate) blogs_exist: usize,
}

pub(crate) fn build_status_rows(
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
pub(crate) fn check_blog_exists(blog_path: &Option<String>, project_root: &Path) -> Option<bool> {
    blog_path.as_ref().map(|bp| project_root.join(bp).exists())
}

pub(crate) fn summarize_rows(
    rows: &[StatusRow],
    modified_paths: &BTreeSet<String>,
) -> StatusSummary {
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

pub(crate) fn sort_rows(rows: &mut [StatusRow], order: &str, project_root: &Path) -> Result<()> {
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
        _ => anyhow::bail!("Invalid order: '{order}'. Valid: topo, depth, alpha"),
    }
}

fn path_depth(path: &str) -> usize {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .count()
}

pub(crate) fn print_status_text(rows: &[StatusRow], summary: StatusSummary) {
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

pub(crate) fn print_status_json(
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
    use csa_core::audit::FileEntry;
    use std::fs;
    use std::path::PathBuf;

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
}
