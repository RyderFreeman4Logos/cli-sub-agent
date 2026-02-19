use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use csa_core::audit::{AuditManifest, AuditStatus, FileEntry};
use csa_core::types::OutputFormat;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::audit::{diff, hash, io, scan, security};
use crate::cli::AuditCommands;

#[derive(Debug, Clone)]
struct StatusRow {
    path: String,
    status: AuditStatus,
    hash: String,
    auditor: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
struct StatusSummary {
    pending: usize,
    generated: usize,
    approved: usize,
    modified: usize,
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
    let mut rows = build_status_rows(&manifest, &current_hashes, &modified_paths);
    let filtered_status = filter.as_deref().map(parse_status).transpose()?;
    if let Some(expected) = filtered_status {
        rows.retain(|row| row.status == expected);
    }

    sort_rows(&mut rows, &order)?;
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
            rows.push(StatusRow {
                path: path.clone(),
                status: effective_status,
                hash: current_hash.clone(),
                auditor: entry.auditor.clone(),
            });
        } else {
            rows.push(StatusRow {
                path: path.clone(),
                status: AuditStatus::Pending,
                hash: current_hash.clone(),
                auditor: None,
            });
        }
    }
    rows
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
    }
    summary
}

fn sort_rows(rows: &mut [StatusRow], order: &str) -> Result<()> {
    match order {
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
        _ => bail!("Invalid order: '{order}'. Valid: depth, alpha"),
    }
}

fn path_depth(path: &str) -> usize {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .count()
}

fn print_status_text(rows: &[StatusRow], summary: StatusSummary) {
    println!(
        "{:<60} | {:<9} | {:<12} | AUDITOR",
        "PATH", "STATUS", "HASH"
    );
    println!("{}", "-".repeat(95));
    for row in rows {
        let short_hash: String = row.hash.chars().take(12).collect();
        let auditor = row.auditor.as_deref().unwrap_or("-");
        println!(
            "{:<60} | {:<9} | {:<12} | {}",
            row.path, row.status, short_hash, auditor
        );
    }
    println!(
        "{} pending, {} generated, {} approved ({} modified since last scan)",
        summary.pending, summary.generated, summary.approved, summary.modified
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
        },
        "files": files,
    });

    println!(
        "{}",
        serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
    );
}
