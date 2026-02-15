use std::path::Path;

use tempfile::TempDir;

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

#[test]
fn parse_source_user_repo() {
    let src = parse_source("alice/my-skill").unwrap();
    assert_eq!(src.url, "https://github.com/alice/my-skill.git");
    assert_eq!(src.name, "my-skill");
    assert!(src.git_ref.is_none());
}

#[test]
fn parse_source_domain_user_repo() {
    let src = parse_source("github.com/bob/audit-tool").unwrap();
    assert_eq!(src.url, "https://github.com/bob/audit-tool.git");
    assert_eq!(src.name, "audit-tool");
}

#[test]
fn parse_source_full_url() {
    let src = parse_source("https://github.com/org/skill-pack").unwrap();
    assert_eq!(src.url, "https://github.com/org/skill-pack.git");
    assert_eq!(src.name, "skill-pack");
}

#[test]
fn parse_source_full_url_with_git_suffix() {
    let src = parse_source("https://github.com/org/tool.git").unwrap();
    assert_eq!(src.url, "https://github.com/org/tool.git");
    assert_eq!(src.name, "tool");
}

#[test]
fn parse_source_with_at_ref() {
    let src = parse_source("alice/my-skill@v2.0").unwrap();
    assert_eq!(src.url, "https://github.com/alice/my-skill.git");
    assert_eq!(src.git_ref, Some("v2.0".to_string()));
}

#[test]
fn parse_source_with_hash_ref() {
    let src = parse_source("alice/my-skill#main").unwrap();
    assert_eq!(src.url, "https://github.com/alice/my-skill.git");
    assert_eq!(src.git_ref, Some("main".to_string()));
}

#[test]
fn parse_source_invalid() {
    assert!(parse_source("").is_err());
    assert!(parse_source("single-word").is_err());
}

#[test]
fn lockfile_round_trip() {
    let lockfile = Lockfile {
        package: vec![
            LockedPackage {
                name: "audit".to_string(),
                repo: "https://github.com/org/audit.git".to_string(),
                commit: "abc123def456".to_string(),
                version: Some("1.0.0".to_string()),
            },
            LockedPackage {
                name: "review".to_string(),
                repo: "https://github.com/org/review.git".to_string(),
                commit: "789abc".to_string(),
                version: None,
            },
        ],
    };

    let tmp = TempDir::new().unwrap();
    let lock_path = tmp.path().join("lock.toml");

    save_lockfile(&lock_path, &lockfile).unwrap();
    let loaded = load_lockfile(&lock_path).unwrap();
    assert_eq!(lockfile, loaded);
}

#[test]
fn upsert_adds_new_package() {
    let mut lockfile = Lockfile {
        package: vec![LockedPackage {
            name: "existing".to_string(),
            repo: "https://example.com/existing.git".to_string(),
            commit: "aaa".to_string(),
            version: None,
        }],
    };

    let new_pkg = LockedPackage {
        name: "new-pkg".to_string(),
        repo: "https://example.com/new.git".to_string(),
        commit: "bbb".to_string(),
        version: Some("2.0".to_string()),
    };

    upsert_package(&mut lockfile, &new_pkg);
    assert_eq!(lockfile.package.len(), 2);
    assert_eq!(lockfile.package[1].name, "new-pkg");
}

#[test]
fn upsert_updates_existing_package() {
    let mut lockfile = Lockfile {
        package: vec![LockedPackage {
            name: "pkg".to_string(),
            repo: "https://example.com/pkg.git".to_string(),
            commit: "old-commit".to_string(),
            version: None,
        }],
    };

    let updated = LockedPackage {
        name: "pkg".to_string(),
        repo: "https://example.com/pkg.git".to_string(),
        commit: "new-commit".to_string(),
        version: Some("3.0".to_string()),
    };

    upsert_package(&mut lockfile, &updated);
    assert_eq!(lockfile.package.len(), 1);
    assert_eq!(lockfile.package[0].commit, "new-commit");
    assert_eq!(lockfile.package[0].version, Some("3.0".to_string()));
}

#[test]
fn cas_dir_is_deterministic() {
    let root = Path::new("/tmp/cache");
    let d1 = cas_dir_for(root, "https://github.com/user/repo.git");
    let d2 = cas_dir_for(root, "https://github.com/user/repo.git");
    assert_eq!(d1, d2);
}

#[test]
fn cas_dir_differs_for_different_urls() {
    let root = Path::new("/tmp/cache");
    let d1 = cas_dir_for(root, "https://github.com/user/repo-a.git");
    let d2 = cas_dir_for(root, "https://github.com/user/repo-b.git");
    assert_ne!(d1, d2);
}

#[test]
fn lock_empty_project() {
    let tmp = TempDir::new().unwrap();
    let lockfile = lock(tmp.path()).unwrap();
    assert!(lockfile.package.is_empty());
}

#[test]
fn lock_picks_up_existing_deps() {
    let tmp = TempDir::new().unwrap();
    let deps = tmp.path().join(".weave").join("deps").join("my-skill");
    std::fs::create_dir_all(&deps).unwrap();
    std::fs::write(deps.join("SKILL.md"), "# My Skill").unwrap();

    let lockfile = lock(tmp.path()).unwrap();
    assert_eq!(lockfile.package.len(), 1);
    assert_eq!(lockfile.package[0].name, "my-skill");
    assert!(lockfile.package[0].repo.is_empty()); // Not installed via weave.
}

#[test]
fn lock_preserves_existing_lockfile_entries() {
    let tmp = TempDir::new().unwrap();

    // Create dep directory.
    let deps = tmp.path().join(".weave").join("deps").join("audit");
    std::fs::create_dir_all(&deps).unwrap();
    std::fs::write(deps.join("SKILL.md"), "# Audit").unwrap();

    // Create initial lockfile with repo info.
    let initial = Lockfile {
        package: vec![LockedPackage {
            name: "audit".to_string(),
            repo: "https://github.com/org/audit.git".to_string(),
            commit: "abc123".to_string(),
            version: Some("1.0".to_string()),
        }],
    };
    let lock_path = tmp.path().join(".weave").join("lock.toml");
    save_lockfile(&lock_path, &initial).unwrap();

    // Re-lock — should preserve the repo/commit info.
    let result = lock(tmp.path()).unwrap();
    assert_eq!(result.package.len(), 1);
    assert_eq!(result.package[0].repo, "https://github.com/org/audit.git");
    assert_eq!(result.package[0].commit, "abc123");
}

#[test]
fn audit_empty_project_no_issues() {
    let tmp = TempDir::new().unwrap();
    let results = audit(tmp.path()).unwrap();
    assert!(results.is_empty());
}

#[test]
fn audit_detects_missing_dep() {
    let tmp = TempDir::new().unwrap();

    // Lockfile references a package that doesn't exist on disk.
    let lockfile = Lockfile {
        package: vec![LockedPackage {
            name: "ghost".to_string(),
            repo: "https://example.com/ghost.git".to_string(),
            commit: "abc".to_string(),
            version: None,
        }],
    };
    let lock_path = tmp.path().join(".weave").join("lock.toml");
    save_lockfile(&lock_path, &lockfile).unwrap();

    let results = audit(tmp.path()).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "ghost");
    assert!(
        results[0]
            .issues
            .iter()
            .any(|i| matches!(i, AuditIssue::MissingFromDeps))
    );
}

#[test]
fn audit_detects_unlocked_dep() {
    let tmp = TempDir::new().unwrap();

    // Create a dep directory but no lockfile entry.
    let deps = tmp.path().join(".weave").join("deps").join("orphan");
    std::fs::create_dir_all(&deps).unwrap();
    std::fs::write(deps.join("SKILL.md"), "# Orphan").unwrap();

    let results = audit(tmp.path()).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "orphan");
    assert!(
        results[0]
            .issues
            .iter()
            .any(|i| matches!(i, AuditIssue::MissingFromLockfile))
    );
}

#[test]
fn audit_detects_missing_skill_md() {
    let tmp = TempDir::new().unwrap();

    // Dep directory exists but has no SKILL.md.
    let deps = tmp.path().join(".weave").join("deps").join("broken");
    std::fs::create_dir_all(&deps).unwrap();

    let lockfile = Lockfile {
        package: vec![LockedPackage {
            name: "broken".to_string(),
            repo: "https://example.com/broken.git".to_string(),
            commit: "abc".to_string(),
            version: None,
        }],
    };
    let lock_path = tmp.path().join(".weave").join("lock.toml");
    save_lockfile(&lock_path, &lockfile).unwrap();

    let results = audit(tmp.path()).unwrap();
    assert_eq!(results.len(), 1);
    assert!(
        results[0]
            .issues
            .iter()
            .any(|i| matches!(i, AuditIssue::MissingSkillMd))
    );
}

#[test]
fn audit_detects_unknown_repo() {
    let tmp = TempDir::new().unwrap();

    let deps = tmp.path().join(".weave").join("deps").join("local");
    std::fs::create_dir_all(&deps).unwrap();
    std::fs::write(deps.join("SKILL.md"), "# Local").unwrap();

    let lockfile = Lockfile {
        package: vec![LockedPackage {
            name: "local".to_string(),
            repo: String::new(),
            commit: String::new(),
            version: None,
        }],
    };
    let lock_path = tmp.path().join(".weave").join("lock.toml");
    save_lockfile(&lock_path, &lockfile).unwrap();

    let results = audit(tmp.path()).unwrap();
    assert_eq!(results.len(), 1);
    assert!(
        results[0]
            .issues
            .iter()
            .any(|i| matches!(i, AuditIssue::UnknownRepo))
    );
}
