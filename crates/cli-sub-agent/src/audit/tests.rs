use super::diff::diff_manifest;
use super::hash::hash_file;
use super::io;
use super::scan::scan_directory;
use super::security::validate_path;
use csa_core::audit::{AuditManifest, AuditStatus, FileEntry, ManifestMeta};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn to_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn contains_path(paths: &[PathBuf], expected: &Path) -> bool {
    paths.iter().any(|path| path == expected)
}

#[test]
fn test_hash_known_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("known.txt");
    fs::write(&path, "hello audit\n").expect("write known file");

    let hash = hash_file(&path).expect("hash should succeed");
    assert_eq!(
        hash,
        "sha256:bec643d1108ea13610b570e988b95dfb0fcbca41effc8e32d543505b330c8c87"
    );
}

#[test]
fn test_scan_respects_gitignore() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(tmp.path().join(".git")).expect("create .git dir");
    fs::write(tmp.path().join(".gitignore"), "ignored.txt\nignored-dir/\n").expect("write ignore");
    fs::write(tmp.path().join("ignored.txt"), "ignored").expect("write ignored file");
    fs::create_dir_all(tmp.path().join("ignored-dir")).expect("create ignored dir");
    fs::write(tmp.path().join("ignored-dir/file.txt"), "ignored dir file")
        .expect("write ignored dir file");
    fs::write(tmp.path().join("keep.txt"), "keep").expect("write keep file");

    let ignores: Vec<String> = vec![];
    let files = scan_directory(tmp.path(), &ignores).expect("scan should succeed");
    assert!(!contains_path(&files, Path::new("ignored.txt")));
    assert!(!contains_path(&files, Path::new("ignored-dir/file.txt")));
    assert!(contains_path(&files, Path::new("keep.txt")));
}

#[test]
fn test_scan_skips_binary() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(tmp.path().join("binary.dat"), [0_u8, 159, 146, 150]).expect("write binary");
    fs::write(tmp.path().join("plain.txt"), "plain text").expect("write text");

    let ignores: Vec<String> = vec![];
    let files = scan_directory(tmp.path(), &ignores).expect("scan should succeed");
    assert!(!contains_path(&files, Path::new("binary.dat")));
    assert!(contains_path(&files, Path::new("plain.txt")));
}

#[test]
fn test_scan_skips_dotgit() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(tmp.path().join(".git")).expect("create .git");
    fs::create_dir_all(tmp.path().join(".csa/audit")).expect("create .csa/audit");
    fs::create_dir_all(tmp.path().join("src")).expect("create src");
    fs::write(tmp.path().join(".git/config"), "core").expect("write .git file");
    fs::write(tmp.path().join(".csa/audit/manifest.toml"), "manifest").expect("write .csa file");
    fs::write(tmp.path().join("src/lib.rs"), "pub fn ok() {}").expect("write src file");

    let ignores: Vec<String> = vec![];
    let files = scan_directory(tmp.path(), &ignores).expect("scan should succeed");
    assert!(!files.iter().any(|path| path.starts_with(".git")));
    assert!(!files.iter().any(|path| path.starts_with(".csa")));
    assert!(contains_path(&files, Path::new("src/lib.rs")));
}

#[test]
fn test_diff_new_file() {
    let manifest = AuditManifest::new(".");
    let mut current = BTreeMap::new();
    current.insert("src/main.rs".to_string(), "sha256:new".to_string());

    let diff = diff_manifest(&manifest, &current);
    assert_eq!(diff.new, vec!["src/main.rs".to_string()]);
    assert!(diff.modified.is_empty());
    assert!(diff.deleted.is_empty());
    assert!(diff.unchanged.is_empty());
}

#[test]
fn test_diff_modified_file() {
    let mut manifest = AuditManifest::new(".");
    manifest.files.insert(
        "src/main.rs".to_string(),
        FileEntry {
            hash: "sha256:old".to_string(),
            audit_status: AuditStatus::Approved,
            blog_path: None,
            auditor: None,
            approved_by: None,
            approved_at: None,
        },
    );
    let mut current = BTreeMap::new();
    current.insert("src/main.rs".to_string(), "sha256:new".to_string());

    let diff = diff_manifest(&manifest, &current);
    assert_eq!(diff.modified, vec!["src/main.rs".to_string()]);
    assert!(diff.new.is_empty());
    assert!(diff.deleted.is_empty());
    assert!(diff.unchanged.is_empty());
}

#[test]
fn test_diff_deleted_file() {
    let mut manifest = AuditManifest::new(".");
    manifest.files.insert(
        "src/main.rs".to_string(),
        FileEntry {
            hash: "sha256:old".to_string(),
            audit_status: AuditStatus::Pending,
            blog_path: None,
            auditor: None,
            approved_by: None,
            approved_at: None,
        },
    );
    let current = BTreeMap::new();

    let diff = diff_manifest(&manifest, &current);
    assert_eq!(diff.deleted, vec!["src/main.rs".to_string()]);
    assert!(diff.new.is_empty());
    assert!(diff.modified.is_empty());
    assert!(diff.unchanged.is_empty());
}

#[test]
fn test_diff_unchanged_file() {
    let mut manifest = AuditManifest::new(".");
    manifest.files.insert(
        "src/main.rs".to_string(),
        FileEntry {
            hash: "sha256:same".to_string(),
            audit_status: AuditStatus::Generated,
            blog_path: None,
            auditor: Some("audit-bot".to_string()),
            approved_by: None,
            approved_at: None,
        },
    );
    let mut current = BTreeMap::new();
    current.insert("src/main.rs".to_string(), "sha256:same".to_string());

    let diff = diff_manifest(&manifest, &current);
    assert_eq!(diff.unchanged, vec!["src/main.rs".to_string()]);
    assert!(diff.new.is_empty());
    assert!(diff.modified.is_empty());
    assert!(diff.deleted.is_empty());
}

#[test]
fn test_security_rejects_absolute() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let absolute = tmp.path().join("abs.txt");
    let result = validate_path(&absolute, tmp.path());
    assert!(result.is_err());
}

#[test]
fn test_security_rejects_parent_traversal() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let result = validate_path(Path::new("../escape.txt"), tmp.path());
    assert!(result.is_err());
}

#[test]
fn test_security_accepts_valid() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let nested = tmp.path().join("nested");
    fs::create_dir_all(&nested).expect("create nested dir");
    let file_path = nested.join("ok.txt");
    fs::write(&file_path, "ok").expect("write file");

    let validated = validate_path(Path::new("nested/ok.txt"), tmp.path()).expect("valid path");
    let canonical = file_path.canonicalize().expect("canonical file");
    assert_eq!(validated, canonical);
}

#[test]
fn test_io_load_nonexistent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("does-not-exist.toml");

    let manifest = io::load(&path).expect("load should return default manifest");
    assert!(manifest.files.is_empty());
    assert_eq!(manifest.meta.version, 1);
    assert_eq!(manifest.meta.project_root, ".");
    assert_eq!(manifest.meta.last_scanned_at, None);
}

#[test]
fn test_io_save_load_round_trip() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join(".csa/audit/manifest.toml");
    let mut files = BTreeMap::new();
    files.insert(
        "src/lib.rs".to_string(),
        FileEntry {
            hash: "sha256:abc".to_string(),
            audit_status: AuditStatus::Generated,
            blog_path: Some("posts/lib.md".to_string()),
            auditor: Some("audit-bot".to_string()),
            approved_by: None,
            approved_at: None,
        },
    );

    let manifest = AuditManifest {
        meta: ManifestMeta {
            version: 1,
            project_root: ".".to_string(),
            created_at: "2026-02-19T00:00:00Z".to_string(),
            updated_at: "2026-02-19T00:00:01Z".to_string(),
            last_scanned_at: Some("2026-02-19T00:00:02Z".to_string()),
        },
        files,
    };

    io::save(&path, &manifest).expect("save should succeed");
    let loaded = io::load(&path).expect("load should succeed");

    let mut expected = manifest.clone();
    expected.meta.updated_at = loaded.meta.updated_at.clone();
    assert_eq!(loaded, expected);
    assert_ne!(loaded.meta.updated_at, manifest.meta.updated_at);

    // Ensure keys remain normalized for path lookups.
    assert!(loaded.files.contains_key(&to_key(Path::new("src/lib.rs"))));
}
