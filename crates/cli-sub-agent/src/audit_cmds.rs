use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use csa_core::audit::{AuditManifest, AuditStatus, FileEntry};
use csa_core::types::OutputFormat;
use std::collections::BTreeSet;
use std::path::Path;

use crate::audit::helpers::{
    canonical_root, compute_mirror_blog_path, current_root, expand_file_args, manifest_path,
    parse_status, resolve_manifest_key, scan_and_hash, validate_mirror_dir,
};
use crate::audit::status::{
    build_status_rows, print_status_json, print_status_text, sort_rows, summarize_rows,
};
use crate::audit::{diff, io, security};
use crate::cli::AuditCommands;

pub(crate) fn handle_audit(command: AuditCommands) -> Result<()> {
    match command {
        AuditCommands::Init {
            root,
            ignore,
            mirror_dir,
        } => handle_audit_init(root, ignore, mirror_dir),
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
            mirror_dir,
        } => handle_audit_update(files, status, auditor, blog_path, mirror_dir),
        AuditCommands::Approve { files, approved_by } => handle_audit_approve(files, approved_by),
        AuditCommands::Reset { files } => handle_audit_reset(files),
        AuditCommands::Sync => handle_audit_sync(),
    }
}

pub(crate) fn handle_audit_init(
    root: String,
    ignores: Vec<String>,
    mirror_dir: Option<String>,
) -> Result<()> {
    let scan_root = canonical_root(Path::new(&root))?;
    let mpath = manifest_path(&scan_root);
    let file_hashes = scan_and_hash(&scan_root, &ignores)?;

    let mut manifest = AuditManifest::new(scan_root.display().to_string());
    manifest.meta.last_scanned_at = Some(Utc::now().to_rfc3339());
    manifest.meta.mirror_dir = mirror_dir.clone();

    // Validate and create mirror directory if specified.
    if let Some(ref dir) = mirror_dir {
        let mirror_path = validate_mirror_dir(dir, &scan_root)?;
        if !mirror_path.exists() {
            std::fs::create_dir_all(&mirror_path).with_context(|| {
                format!(
                    "Failed to create mirror directory: {}",
                    mirror_path.display()
                )
            })?;
        }
    }

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

    io::save(&mpath, &manifest)?;
    println!(
        "Initialized audit manifest: {} ({} files)",
        mpath.display(),
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
    let mpath = manifest_path(&root);
    let manifest = io::load(&mpath)?;
    let current_hashes = scan_and_hash(&root, &[])?;
    let manifest_diff = diff::diff_manifest(&manifest, &current_hashes);

    let modified_paths: BTreeSet<String> = manifest_diff.modified.into_iter().collect();
    let mut rows = build_status_rows(&manifest, &current_hashes, &modified_paths, &root);
    let filtered_status = filter.as_deref().map(parse_status).transpose()?;
    if let Some(expected) = filtered_status {
        rows.retain(|row| row.status == expected);
    }

    let all_keys: Vec<String> = current_hashes.keys().cloned().collect();
    sort_rows(&mut rows, &order, &root, &all_keys)?;
    let summary = summarize_rows(&rows, &modified_paths);

    match format {
        OutputFormat::Text => {
            print_status_text(&rows, summary);
        }
        OutputFormat::Json => {
            print_status_json(&manifest, &mpath, &rows, summary);
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
    mirror_dir: Option<String>,
) -> Result<()> {
    let status = parse_status(&status_str)?;
    let root = current_root()?;
    let path = manifest_path(&root);
    let mut manifest = io::load(&path)?;

    // Validate and apply CLI mirror_dir override.
    if let Some(ref md) = mirror_dir {
        validate_mirror_dir(md, &root)?;
        manifest.meta.mirror_dir = Some(md.clone());
    }

    // Resolve effective mirror_dir: CLI flag takes priority, then manifest meta.
    let effective_mirror_dir = mirror_dir
        .as_deref()
        .or(manifest.meta.mirror_dir.as_deref());

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

        // Blog path resolution: explicit --blog-path wins, otherwise auto-compute
        // from effective mirror_dir if available.
        entry.blog_path = if blog_path.is_some() {
            blog_path.clone()
        } else {
            effective_mirror_dir.map(|md| compute_mirror_blog_path(md, &key))
        };

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::helpers::canonical_root;
    use std::fs;

    #[test]
    fn test_audit_init_stores_mirror_dir_in_manifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();

        // Create a dummy source file so the manifest is non-trivial.
        let src_dir = root.join("src");
        fs::create_dir_all(&src_dir).expect("create src dir");
        fs::write(src_dir.join("lib.rs"), "fn main() {}").expect("write src");

        // Simulate handle_audit_init with mirror_dir.
        let scan_root = canonical_root(root).unwrap();
        let mpath = manifest_path(&scan_root);
        let file_hashes = scan_and_hash(&scan_root, &[]).unwrap();

        let mut manifest = AuditManifest::new(scan_root.display().to_string());
        manifest.meta.last_scanned_at = Some(Utc::now().to_rfc3339());
        manifest.meta.mirror_dir = Some("./drafts".to_string());

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

        io::save(&mpath, &manifest).expect("save");

        // Reload and verify.
        let loaded = io::load(&mpath).expect("load");
        assert_eq!(loaded.meta.mirror_dir, Some("./drafts".to_string()));
    }

    #[test]
    fn test_audit_init_creates_mirror_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();

        // Create a dummy source file.
        fs::create_dir_all(root.join("src")).expect("create src");
        fs::write(root.join("src/lib.rs"), "fn main() {}").expect("write src");

        let mirror_path = root.join("my-drafts");
        assert!(!mirror_path.exists(), "mirror dir should not exist yet");

        handle_audit_init(
            root.to_string_lossy().to_string(),
            vec![],
            Some("my-drafts".to_string()),
        )
        .expect("init should succeed");

        assert!(mirror_path.exists(), "mirror dir should have been created");
    }

    #[test]
    fn test_audit_update_auto_computes_blog_path_from_mirror_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();

        // Initialize a manifest with mirror_dir in meta.
        fs::create_dir_all(root.join("src")).expect("create src");
        fs::write(root.join("src/lib.rs"), "fn main() {}").expect("write src");

        handle_audit_init(
            root.to_string_lossy().to_string(),
            vec![],
            Some("./drafts".to_string()),
        )
        .expect("init");

        // Run update from the project root so current_root() resolves correctly.
        let _guard = TempCwd::set(root);
        handle_audit_update(
            vec!["src/lib.rs".to_string()],
            "generated".to_string(),
            None,
            None, // no explicit blog_path
            None, // no CLI mirror_dir override
        )
        .expect("update");

        let scan_root = canonical_root(root).unwrap();
        let manifest = io::load(&manifest_path(&scan_root)).expect("load");
        let entry = manifest
            .files
            .get("src/lib.rs")
            .expect("entry should exist");
        assert_eq!(
            entry.blog_path,
            Some("drafts/src/lib.rs.md".to_string()),
            "blog_path should be auto-computed from manifest mirror_dir"
        );
    }

    #[test]
    fn test_audit_update_explicit_blog_path_overrides_mirror_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();

        fs::create_dir_all(root.join("src")).expect("create src");
        fs::write(root.join("src/lib.rs"), "fn main() {}").expect("write src");

        handle_audit_init(
            root.to_string_lossy().to_string(),
            vec![],
            Some("./drafts".to_string()),
        )
        .expect("init");

        let _guard = TempCwd::set(root);
        handle_audit_update(
            vec!["src/lib.rs".to_string()],
            "generated".to_string(),
            None,
            Some("custom/blog.md".to_string()), // explicit blog_path
            None,
        )
        .expect("update");

        let scan_root = canonical_root(root).unwrap();
        let manifest = io::load(&manifest_path(&scan_root)).expect("load");
        let entry = manifest
            .files
            .get("src/lib.rs")
            .expect("entry should exist");
        assert_eq!(
            entry.blog_path,
            Some("custom/blog.md".to_string()),
            "explicit --blog-path should override auto-computation"
        );
    }

    #[test]
    fn test_audit_update_cli_mirror_dir_overrides_manifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();

        fs::create_dir_all(root.join("src")).expect("create src");
        fs::write(root.join("src/lib.rs"), "fn main() {}").expect("write src");

        // Init without mirror_dir.
        handle_audit_init(root.to_string_lossy().to_string(), vec![], None).expect("init");

        let _guard = TempCwd::set(root);
        handle_audit_update(
            vec!["src/lib.rs".to_string()],
            "generated".to_string(),
            None,
            None,
            Some("output".to_string()), // CLI mirror_dir flag
        )
        .expect("update");

        let scan_root = canonical_root(root).unwrap();
        let manifest = io::load(&manifest_path(&scan_root)).expect("load");

        // manifest.meta.mirror_dir should be updated by CLI flag.
        assert_eq!(manifest.meta.mirror_dir, Some("output".to_string()));

        let entry = manifest
            .files
            .get("src/lib.rs")
            .expect("entry should exist");
        assert_eq!(
            entry.blog_path,
            Some("output/src/lib.rs.md".to_string()),
            "blog_path should be auto-computed from CLI --mirror-dir"
        );
    }

    /// RAII guard for temporarily changing the working directory in tests.
    ///
    /// Restores to a known-good stable directory on drop, not the previous cwd,
    /// to avoid failures when parallel tests remove each other's temp directories.
    struct TempCwd;

    impl TempCwd {
        fn set(new_dir: &Path) -> Self {
            std::env::set_current_dir(new_dir).expect("set cwd");
            Self
        }
    }

    impl Drop for TempCwd {
        fn drop(&mut self) {
            // Restore to a stable directory that always exists.
            let _ = std::env::set_current_dir("/tmp");
        }
    }
}
