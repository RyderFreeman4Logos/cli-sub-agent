use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::package::AuditIssue;

use super::*;

// ---------------------------------------------------------------------------
// check_symlinks tests
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn make_symlink(link: &Path, target: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(unix)]
#[test]
fn check_finds_broken_symlinks() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = tmp.path().join("skills");
    std::fs::create_dir_all(&skill_dir).unwrap();

    // Create a broken symlink (target doesn't exist).
    let link = skill_dir.join("broken-skill");
    make_symlink(&link, Path::new("/nonexistent/path/to/skill"));

    let results = check_symlinks(tmp.path(), &[PathBuf::from("skills")], false).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].issues.len(), 1);
    assert!(matches!(
        &results[0].issues[0],
        AuditIssue::BrokenSymlink { path, .. } if path == &link
    ));
    assert_eq!(results[0].fixed, 0);
}

#[cfg(unix)]
#[test]
fn check_preserves_valid_symlinks() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = tmp.path().join("skills");
    std::fs::create_dir_all(&skill_dir).unwrap();

    // Create a valid target and symlink.
    let target = tmp.path().join("real-skill");
    std::fs::create_dir_all(&target).unwrap();
    let link = skill_dir.join("good-skill");
    make_symlink(&link, &target);

    let results = check_symlinks(tmp.path(), &[PathBuf::from("skills")], false).unwrap();
    // No issues found — result list is empty (only populated when issues exist).
    assert!(results.is_empty());
    // Symlink still exists.
    assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
}

#[cfg(unix)]
#[test]
fn check_fix_removes_broken_symlinks() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = tmp.path().join("skills");
    std::fs::create_dir_all(&skill_dir).unwrap();

    // Create broken and valid symlinks.
    let broken = skill_dir.join("broken");
    make_symlink(&broken, Path::new("/nonexistent"));

    let target = tmp.path().join("real");
    std::fs::create_dir_all(&target).unwrap();
    let valid = skill_dir.join("valid");
    make_symlink(&valid, &target);

    let results = check_symlinks(tmp.path(), &[PathBuf::from("skills")], true).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].fixed, 1);

    // Broken symlink removed, valid symlink preserved.
    assert!(!broken.exists() && broken.symlink_metadata().is_err());
    assert!(valid.symlink_metadata().unwrap().file_type().is_symlink());
}

#[cfg(unix)]
#[test]
fn check_skips_nonexistent_directories() {
    let tmp = TempDir::new().unwrap();
    let results = check_symlinks(tmp.path(), &[PathBuf::from("does-not-exist")], false).unwrap();
    assert!(results.is_empty());
}

#[cfg(unix)]
#[test]
fn check_ignores_regular_files() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = tmp.path().join("skills");
    std::fs::create_dir_all(&skill_dir).unwrap();

    // Regular file — should not be touched.
    std::fs::write(skill_dir.join("not-a-link"), "content").unwrap();

    let results = check_symlinks(tmp.path(), &[PathBuf::from("skills")], true).unwrap();
    assert!(results.is_empty());
    assert!(skill_dir.join("not-a-link").exists());
}
