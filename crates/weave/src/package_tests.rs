use std::process::Command;
use tempfile::TempDir;

use super::package_git::cas_dir_for;
use super::*;

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
    let lockfile = Lockfile::with_packages(vec![
        LockedPackage {
            name: "audit".to_string(),
            repo: "https://github.com/org/audit.git".to_string(),
            commit: "abc123def456".to_string(),
            version: Some("1.0.0".to_string()),
            source_kind: SourceKind::default(),
            requested_version: None,
            resolved_ref: None,
        },
        LockedPackage {
            name: "review".to_string(),
            repo: "https://github.com/org/review.git".to_string(),
            commit: "789abc".to_string(),
            version: None,
            source_kind: SourceKind::default(),
            requested_version: None,
            resolved_ref: None,
        },
    ]);

    let tmp = TempDir::new().unwrap();
    let lock_path = tmp.path().join("lock.toml");

    save_lockfile(&lock_path, &lockfile).unwrap();
    let loaded = load_lockfile(&lock_path).unwrap();
    assert_eq!(lockfile, loaded);
}

#[test]
fn upsert_adds_new_package() {
    let mut lockfile = Lockfile::with_packages(vec![LockedPackage {
        name: "existing".to_string(),
        repo: "https://example.com/existing.git".to_string(),
        commit: "aaa".to_string(),
        version: None,
        source_kind: SourceKind::default(),
        requested_version: None,
        resolved_ref: None,
    }]);

    let new_pkg = LockedPackage {
        name: "new-pkg".to_string(),
        repo: "https://example.com/new.git".to_string(),
        commit: "bbb".to_string(),
        version: Some("2.0".to_string()),
        source_kind: SourceKind::default(),
        requested_version: None,
        resolved_ref: None,
    };

    upsert_package(&mut lockfile, &new_pkg);
    assert_eq!(lockfile.package.len(), 2);
    assert_eq!(lockfile.package[1].name, "new-pkg");
}

#[test]
fn upsert_updates_existing_package() {
    let mut lockfile = Lockfile::with_packages(vec![LockedPackage {
        name: "pkg".to_string(),
        repo: "https://example.com/pkg.git".to_string(),
        commit: "old-commit".to_string(),
        version: None,
        source_kind: SourceKind::default(),
        requested_version: None,
        resolved_ref: None,
    }]);

    let updated = LockedPackage {
        name: "pkg".to_string(),
        repo: "https://example.com/pkg.git".to_string(),
        commit: "new-commit".to_string(),
        version: Some("3.0".to_string()),
        source_kind: SourceKind::default(),
        requested_version: None,
        resolved_ref: None,
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
fn ensure_cached_fetch_updates_default_head_commit() {
    fn run_git(current_dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(current_dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    let tmp = TempDir::new().unwrap();
    let work = tmp.path().join("work");
    let remote = tmp.path().join("remote.git");
    let cache = tmp.path().join("cache");
    std::fs::create_dir_all(&work).unwrap();

    run_git(&work, &["init", "--quiet"]);
    run_git(&work, &["config", "user.email", "test@example.com"]);
    run_git(&work, &["config", "user.name", "Test User"]);
    std::fs::write(work.join("README.md"), "v1\n").unwrap();
    run_git(&work, &["add", "README.md"]);
    run_git(&work, &["commit", "--quiet", "-m", "initial"]);
    run_git(&work, &["branch", "-M", "main"]);

    let clone_status = Command::new("git")
        .args(["clone", "--bare", "--quiet"])
        .arg(&work)
        .arg(&remote)
        .status()
        .unwrap();
    assert!(clone_status.success(), "failed to clone bare remote");

    run_git(
        &work,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );

    let cas = ensure_cached(&cache, remote.to_str().unwrap()).unwrap();
    let first = resolve_commit(&cas, None).unwrap();

    std::fs::write(work.join("README.md"), "v2\n").unwrap();
    run_git(&work, &["add", "README.md"]);
    run_git(&work, &["commit", "--quiet", "-m", "second"]);
    let expected = run_git(&work, &["rev-parse", "HEAD"]);
    run_git(&work, &["push", "--quiet", "origin", "main"]);

    let cas_after = ensure_cached(&cache, remote.to_str().unwrap()).unwrap();
    assert_eq!(cas_after, cas);
    let second = resolve_commit(&cas, None).unwrap();

    assert_ne!(first, second);
    assert_eq!(second, expected);
}

#[test]
fn lock_empty_project() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");
    let lockfile = lock(tmp.path(), &store).unwrap();
    assert!(lockfile.package.is_empty());
    assert!(tmp.path().join("weave.lock").is_file());
}

#[test]
fn lockfile_path_returns_weave_lock() {
    let root = Path::new("/tmp/project");
    assert_eq!(lockfile_path(root), Path::new("/tmp/project/weave.lock"));
}

#[test]
fn find_lockfile_prefers_new_path() {
    let tmp = TempDir::new().unwrap();
    let old = tmp.path().join(".weave").join("lock.toml");
    std::fs::create_dir_all(old.parent().unwrap()).unwrap();
    std::fs::write(&old, "[[package]]").unwrap();
    let new = tmp.path().join("weave.lock");
    std::fs::write(&new, "[[package]]").unwrap();

    let found = find_lockfile(tmp.path()).unwrap();
    assert_eq!(found, new, "should prefer weave.lock over .weave/lock.toml");
}

#[test]
fn find_lockfile_falls_back_to_legacy() {
    let tmp = TempDir::new().unwrap();
    let old = tmp.path().join(".weave").join("lock.toml");
    std::fs::create_dir_all(old.parent().unwrap()).unwrap();
    std::fs::write(&old, "[[package]]").unwrap();

    let found = find_lockfile(tmp.path()).unwrap();
    assert_eq!(found, old, "should fall back to .weave/lock.toml");
}

#[test]
fn find_lockfile_returns_none_when_missing() {
    let tmp = TempDir::new().unwrap();
    assert!(find_lockfile(tmp.path()).is_none());
}

#[test]
fn lock_reads_from_legacy_and_writes_to_new() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");
    let checkout = package_dir(&store, "migrated", "abc123").unwrap();
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(checkout.join("SKILL.md"), "# Migrated").unwrap();
    let legacy = tmp.path().join(".weave").join("lock.toml");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    let initial = Lockfile::with_packages(vec![LockedPackage {
        name: "migrated".to_string(),
        repo: "https://github.com/org/migrated.git".to_string(),
        commit: "abc123".to_string(),
        version: None,
        source_kind: SourceKind::Git,
        requested_version: None,
        resolved_ref: None,
    }]);
    save_lockfile(&legacy, &initial).unwrap();
    let result = lock(tmp.path(), &store).unwrap();
    assert_eq!(result.package.len(), 1);
    assert_eq!(
        result.package[0].repo,
        "https://github.com/org/migrated.git"
    );
    assert!(tmp.path().join("weave.lock").is_file());
}

#[test]
fn global_store_root_returns_expected_path() {
    let root = global_store_root().unwrap();
    assert!(root.ends_with("weave/packages"), "got: {}", root.display());
}

#[test]
fn package_dir_uses_commit_prefix() {
    let store = Path::new("/store");
    let dir = package_dir(store, "my-skill", "abcdef1234567890").unwrap();
    assert_eq!(dir, Path::new("/store/my-skill/abcdef12"));
}

#[test]
fn package_dir_short_commit_uses_full_hash() {
    let store = Path::new("/store");
    let dir = package_dir(store, "skill", "abc").unwrap();
    assert_eq!(dir, Path::new("/store/skill/abc"));
}

#[test]
fn is_checkout_valid_empty_dir_is_invalid() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("empty");
    std::fs::create_dir_all(&dir).unwrap();
    assert!(!is_checkout_valid(&dir));
}

#[test]
fn is_checkout_valid_nonempty_dir_is_valid() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("valid");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), "# Skill").unwrap();
    assert!(is_checkout_valid(&dir));
}

#[test]
fn is_checkout_valid_missing_dir_is_invalid() {
    assert!(!is_checkout_valid(Path::new("/nonexistent/path")));
}

#[test]
fn lock_preserves_existing_lockfile_entries() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");
    let checkout = package_dir(&store, "audit", "abc123").unwrap();
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(checkout.join("SKILL.md"), "# Audit").unwrap();
    let initial = Lockfile::with_packages(vec![LockedPackage {
        name: "audit".to_string(),
        repo: "https://github.com/org/audit.git".to_string(),
        commit: "abc123".to_string(),
        version: Some("1.0".to_string()),
        source_kind: SourceKind::default(),
        requested_version: None,
        resolved_ref: None,
    }]);
    let lp = lockfile_path(tmp.path());
    save_lockfile(&lp, &initial).unwrap();
    let result = lock(tmp.path(), &store).unwrap();
    assert_eq!(result.package.len(), 1);
    assert_eq!(result.package[0].repo, "https://github.com/org/audit.git");
    assert_eq!(result.package[0].commit, "abc123");
}

#[test]
fn source_kind_serde_roundtrip() {
    let lockfile = Lockfile::with_packages(vec![
        LockedPackage {
            name: "from-git".to_string(),
            repo: "https://github.com/org/from-git.git".to_string(),
            commit: "abc123".to_string(),
            version: None,
            source_kind: SourceKind::Git,
            requested_version: None,
            resolved_ref: None,
        },
        LockedPackage {
            name: "from-local".to_string(),
            repo: String::new(),
            commit: String::new(),
            version: Some("1.0".to_string()),
            source_kind: SourceKind::Local,
            requested_version: None,
            resolved_ref: None,
        },
    ]);

    let tmp = TempDir::new().unwrap();
    let lock_path = tmp.path().join("lock.toml");

    save_lockfile(&lock_path, &lockfile).unwrap();
    let loaded = load_lockfile(&lock_path).unwrap();
    assert_eq!(lockfile, loaded);
    assert_eq!(loaded.package[0].source_kind, SourceKind::Git);
    assert_eq!(loaded.package[1].source_kind, SourceKind::Local);
}

#[test]
fn source_kind_defaults_to_git_for_old_lockfiles() {
    let toml_str = r#"
[[package]]
name = "legacy"
repo = "https://github.com/org/legacy.git"
commit = "abc"
"#;
    let lockfile: Lockfile = toml::from_str(toml_str).unwrap();
    assert_eq!(lockfile.package[0].source_kind, SourceKind::Git);
}

#[test]
fn lock_preserves_source_kind_from_existing_lockfile() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");
    let checkout = package_dir(&store, "local-dep", "local").unwrap();
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(checkout.join("SKILL.md"), "# Local").unwrap();
    let initial = Lockfile::with_packages(vec![LockedPackage {
        name: "local-dep".to_string(),
        repo: String::new(),
        commit: String::new(),
        version: None,
        source_kind: SourceKind::Local,
        requested_version: None,
        resolved_ref: None,
    }]);
    let lp = lockfile_path(tmp.path());
    save_lockfile(&lp, &initial).unwrap();
    let result = lock(tmp.path(), &store).unwrap();
    assert_eq!(result.package.len(), 1);
    assert_eq!(result.package[0].source_kind, SourceKind::Local);
}

#[test]
fn lockfile_preserves_versions_section_on_roundtrip() {
    // When a weave.lock contains both [versions] and [[package]], the Lockfile
    // parser must preserve the [versions] data across load/save.
    let toml_str = r#"
[versions]
csa = "0.1.32"
weave = "0.1.32"

[migrations]
applied = []

[[package]]
name = "audit"
repo = "https://github.com/org/audit.git"
commit = "abc123"
"#;
    let lockfile: Lockfile = toml::from_str(toml_str).unwrap();
    assert_eq!(lockfile.package.len(), 1);
    assert!(lockfile.versions.is_some());
    assert!(lockfile.migrations.is_some());

    let tmp = TempDir::new().unwrap();
    let lock_path = tmp.path().join("weave.lock");
    save_lockfile(&lock_path, &lockfile).unwrap();

    let loaded = load_lockfile(&lock_path).unwrap();
    assert_eq!(loaded.package.len(), 1);
    assert!(loaded.versions.is_some(), "versions section lost on save");
    assert!(
        loaded.migrations.is_some(),
        "migrations section lost on save"
    );
}

#[test]
fn lockfile_parses_package_only_format() {
    // Lockfile must parse a file with only [[package]] entries (no [versions]).
    let toml_str = r#"
[[package]]
name = "my-skill"
repo = "https://github.com/org/my-skill.git"
commit = "abc123"
"#;
    let lockfile: Lockfile = toml::from_str(toml_str).unwrap();
    assert_eq!(lockfile.package.len(), 1);
    assert!(lockfile.versions.is_none());
    assert!(lockfile.migrations.is_none());
}

#[test]
fn lockfile_parses_versions_only_format() {
    // Lockfile must parse a file with only [versions] (no [[package]]).
    let toml_str = r#"
[versions]
csa = "0.1.32"
weave = "0.1.32"

[migrations]
applied = []
"#;
    let lockfile: Lockfile = toml::from_str(toml_str).unwrap();
    assert!(lockfile.package.is_empty());
    assert!(lockfile.versions.is_some());
}
