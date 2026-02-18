use tempfile::TempDir;

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
    let lockfile = Lockfile {
        package: vec![
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
            source_kind: SourceKind::default(),
            requested_version: None,
            resolved_ref: None,
        }],
    };

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
    let mut lockfile = Lockfile {
        package: vec![LockedPackage {
            name: "pkg".to_string(),
            repo: "https://example.com/pkg.git".to_string(),
            commit: "old-commit".to_string(),
            version: None,
            source_kind: SourceKind::default(),
            requested_version: None,
            resolved_ref: None,
        }],
    };

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
fn lock_empty_project() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");
    let lockfile = lock(tmp.path(), &store).unwrap();
    assert!(lockfile.package.is_empty());
    // Lockfile written to new path.
    assert!(tmp.path().join("weave.lock").is_file());
}

// ---------------------------------------------------------------------------
// Lockfile path migration tests
// ---------------------------------------------------------------------------

#[test]
fn lockfile_path_returns_weave_lock() {
    let root = Path::new("/tmp/project");
    assert_eq!(lockfile_path(root), Path::new("/tmp/project/weave.lock"));
}

#[test]
fn find_lockfile_prefers_new_path() {
    let tmp = TempDir::new().unwrap();
    // Create both old and new lockfiles.
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
    // Only legacy lockfile exists.
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

    // Create package checkout in global store.
    let checkout = package_dir(&store, "migrated", "abc123").unwrap();
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(checkout.join("SKILL.md"), "# Migrated").unwrap();

    // Write lockfile only at the legacy path.
    let legacy = tmp.path().join(".weave").join("lock.toml");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    let initial = Lockfile {
        package: vec![LockedPackage {
            name: "migrated".to_string(),
            repo: "https://github.com/org/migrated.git".to_string(),
            commit: "abc123".to_string(),
            version: None,
            source_kind: SourceKind::Git,
            requested_version: None,
            resolved_ref: None,
        }],
    };
    save_lockfile(&legacy, &initial).unwrap();

    // Re-lock reads from legacy, writes to new.
    let result = lock(tmp.path(), &store).unwrap();
    assert_eq!(result.package.len(), 1);
    assert_eq!(
        result.package[0].repo,
        "https://github.com/org/migrated.git"
    );

    // New lockfile was created.
    assert!(tmp.path().join("weave.lock").is_file());
}

// ---------------------------------------------------------------------------
// Global package store tests
// ---------------------------------------------------------------------------

#[test]
fn global_store_root_returns_expected_path() {
    let root = global_store_root().unwrap();
    // Must end with weave/packages under some data directory.
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

    // Create package checkout in global store.
    let checkout = package_dir(&store, "audit", "abc123").unwrap();
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(checkout.join("SKILL.md"), "# Audit").unwrap();

    // Create initial lockfile at the new path.
    let initial = Lockfile {
        package: vec![LockedPackage {
            name: "audit".to_string(),
            repo: "https://github.com/org/audit.git".to_string(),
            commit: "abc123".to_string(),
            version: Some("1.0".to_string()),
            source_kind: SourceKind::default(),
            requested_version: None,
            resolved_ref: None,
        }],
    };
    let lp = lockfile_path(tmp.path());
    save_lockfile(&lp, &initial).unwrap();

    // Re-lock — should preserve the repo/commit info.
    let result = lock(tmp.path(), &store).unwrap();
    assert_eq!(result.package.len(), 1);
    assert_eq!(result.package[0].repo, "https://github.com/org/audit.git");
    assert_eq!(result.package[0].commit, "abc123");
}

#[test]
fn audit_empty_project_no_issues() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");
    let results = audit(tmp.path(), &store).unwrap();
    assert!(results.is_empty());
}

#[test]
fn audit_detects_missing_dep() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");

    // Lockfile references a package that doesn't exist in the store.
    let lockfile = Lockfile {
        package: vec![LockedPackage {
            name: "ghost".to_string(),
            repo: "https://example.com/ghost.git".to_string(),
            commit: "abc12345".to_string(),
            version: None,
            source_kind: SourceKind::default(),
            requested_version: None,
            resolved_ref: None,
        }],
    };
    let lp = lockfile_path(tmp.path());
    save_lockfile(&lp, &lockfile).unwrap();

    let results = audit(tmp.path(), &store).unwrap();
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
fn audit_detects_missing_skill_md() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");

    // Create checkout dir in store but without SKILL.md.
    let checkout = package_dir(&store, "broken", "abc12345").unwrap();
    std::fs::create_dir_all(&checkout).unwrap();

    let lockfile = Lockfile {
        package: vec![LockedPackage {
            name: "broken".to_string(),
            repo: "https://example.com/broken.git".to_string(),
            commit: "abc12345".to_string(),
            version: None,
            source_kind: SourceKind::default(),
            requested_version: None,
            resolved_ref: None,
        }],
    };
    let lp = lockfile_path(tmp.path());
    save_lockfile(&lp, &lockfile).unwrap();

    let results = audit(tmp.path(), &store).unwrap();
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
    let store = tmp.path().join("store");

    let checkout = package_dir(&store, "local", "abc12345").unwrap();
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(checkout.join("SKILL.md"), "# Local").unwrap();

    let lockfile = Lockfile {
        package: vec![LockedPackage {
            name: "local".to_string(),
            repo: String::new(),
            commit: "abc12345".to_string(),
            version: None,
            source_kind: SourceKind::Git, // Git source with empty repo → UnknownRepo
            requested_version: None,
            resolved_ref: None,
        }],
    };
    let lp = lockfile_path(tmp.path());
    save_lockfile(&lp, &lockfile).unwrap();

    let results = audit(tmp.path(), &store).unwrap();
    assert_eq!(results.len(), 1);
    assert!(
        results[0]
            .issues
            .iter()
            .any(|i| matches!(i, AuditIssue::UnknownRepo))
    );
}

// ---------------------------------------------------------------------------
// SKILL.md case-mismatch detection tests
// ---------------------------------------------------------------------------

#[test]
fn audit_detects_case_mismatch_skill_md() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");

    // Checkout in store with lowercase `skill.md` instead of `SKILL.md`.
    let checkout = package_dir(&store, "bad-case", "abc12345").unwrap();
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(checkout.join("skill.md"), "# Wrong Case").unwrap();

    // Skip on case-insensitive filesystems (e.g. macOS HFS+/APFS default).
    let probe = checkout.join("_CaSe_PrObE_");
    std::fs::write(&probe, "").unwrap();
    if checkout.join("_case_probe_").exists() {
        std::fs::remove_file(&probe).unwrap();
        return;
    }
    std::fs::remove_file(&probe).unwrap();

    let lockfile = Lockfile {
        package: vec![LockedPackage {
            name: "bad-case".to_string(),
            repo: "https://example.com/bad-case.git".to_string(),
            commit: "abc12345".to_string(),
            version: None,
            source_kind: SourceKind::default(),
            requested_version: None,
            resolved_ref: None,
        }],
    };
    let lp = lockfile_path(tmp.path());
    save_lockfile(&lp, &lockfile).unwrap();

    let results = audit(tmp.path(), &store).unwrap();
    assert_eq!(results.len(), 1);
    assert!(
        results[0]
            .issues
            .iter()
            .any(|i| matches!(i, AuditIssue::CaseMismatchSkillMd { found } if found == "skill.md")),
        "expected CaseMismatchSkillMd, got: {:?}",
        results[0].issues
    );
    // Verify the display message is helpful.
    let msg = results[0].issues[0].to_string();
    assert!(
        msg.contains("skill.md") && msg.contains("SKILL.md") && msg.contains("Rename"),
        "unhelpful message: {msg}"
    );
}

#[test]
fn audit_correct_skill_md_no_case_issue() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");

    // Checkout with the correct `SKILL.md`.
    let checkout = package_dir(&store, "good", "abc12345").unwrap();
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(checkout.join("SKILL.md"), "# Good Skill").unwrap();

    let lockfile = Lockfile {
        package: vec![LockedPackage {
            name: "good".to_string(),
            repo: "https://example.com/good.git".to_string(),
            commit: "abc12345".to_string(),
            version: None,
            source_kind: SourceKind::default(),
            requested_version: None,
            resolved_ref: None,
        }],
    };
    let lp = lockfile_path(tmp.path());
    save_lockfile(&lp, &lockfile).unwrap();

    let results = audit(tmp.path(), &store).unwrap();
    assert!(results.is_empty(), "expected no issues, got: {results:?}");
}

#[test]
fn audit_neither_skill_md_variant_is_missing() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");

    // Checkout exists but has NO skill.md variant at all.
    let checkout = package_dir(&store, "empty", "abc12345").unwrap();
    std::fs::create_dir_all(&checkout).unwrap();

    let lockfile = Lockfile {
        package: vec![LockedPackage {
            name: "empty".to_string(),
            repo: "https://example.com/empty.git".to_string(),
            commit: "abc12345".to_string(),
            version: None,
            source_kind: SourceKind::default(),
            requested_version: None,
            resolved_ref: None,
        }],
    };
    let lp = lockfile_path(tmp.path());
    save_lockfile(&lp, &lockfile).unwrap();

    let results = audit(tmp.path(), &store).unwrap();
    assert_eq!(results.len(), 1);
    assert!(
        results[0]
            .issues
            .iter()
            .any(|i| matches!(i, AuditIssue::MissingSkillMd)),
        "expected MissingSkillMd, got: {:?}",
        results[0].issues
    );
}

// ---------------------------------------------------------------------------
// migrate tests
// ---------------------------------------------------------------------------

#[test]
fn migrate_nothing_when_no_legacy_lockfile() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let store = tmp.path().join("store");
    let result = migrate(tmp.path(), &cache, &store).unwrap();
    assert_eq!(result, MigrateResult::NothingToMigrate);
}

#[test]
fn migrate_already_migrated_when_weave_lock_exists() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let store = tmp.path().join("store");

    // Create weave.lock (new format).
    let new = lockfile_path(tmp.path());
    std::fs::write(&new, "").unwrap();

    let result = migrate(tmp.path(), &cache, &store).unwrap();
    assert_eq!(result, MigrateResult::AlreadyMigrated);
}

#[test]
fn migrate_creates_weave_lock_from_legacy() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let store = tmp.path().join("store");

    // Create checkout that already exists in global store (skip git ops).
    let checkout = package_dir(&store, "test-skill", "abc12345").unwrap();
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(checkout.join("SKILL.md"), "# Test").unwrap();

    // Write legacy lockfile with a Local source (no git needed).
    let legacy = tmp.path().join(".weave").join("lock.toml");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    let lockfile = Lockfile {
        package: vec![LockedPackage {
            name: "test-skill".to_string(),
            repo: String::new(),
            commit: String::new(),
            version: None,
            source_kind: SourceKind::Local,
            requested_version: None,
            resolved_ref: None,
        }],
    };
    save_lockfile(&legacy, &lockfile).unwrap();

    let result = migrate(tmp.path(), &cache, &store).unwrap();
    assert!(
        matches!(result, MigrateResult::Migrated { count: 1, .. }),
        "expected Migrated with 1 package, got: {result:?}"
    );

    // New lockfile must exist.
    let new_path = lockfile_path(tmp.path());
    assert!(new_path.is_file(), "weave.lock should be created");

    // Verify content matches.
    let loaded = load_lockfile(&new_path).unwrap();
    assert_eq!(loaded.package.len(), 1);
    assert_eq!(loaded.package[0].name, "test-skill");
}

#[test]
fn migrate_skips_valid_checkout_in_global_store() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    let store = tmp.path().join("store");

    // Pre-create a valid checkout in global store.
    let checkout = package_dir(&store, "pre-existing", "deadbeef").unwrap();
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(checkout.join("SKILL.md"), "# Pre-existing").unwrap();

    // Write legacy lockfile referencing git source with empty repo
    // (which will be skipped because repo is empty).
    let legacy = tmp.path().join(".weave").join("lock.toml");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    let lockfile = Lockfile {
        package: vec![LockedPackage {
            name: "pre-existing".to_string(),
            repo: "https://example.com/pre-existing.git".to_string(),
            commit: "deadbeef".to_string(),
            version: None,
            source_kind: SourceKind::Git,
            requested_version: None,
            resolved_ref: None,
        }],
    };
    save_lockfile(&legacy, &lockfile).unwrap();

    // This should succeed because checkout already valid — no git needed.
    let result = migrate(tmp.path(), &cache, &store).unwrap();
    assert!(
        matches!(
            result,
            MigrateResult::Migrated {
                count: 1,
                checkouts: 0,
                ..
            }
        ),
        "expected Migrated(count=1, checkouts=0) since checkout valid, got: {result:?}"
    );

    // New lockfile must exist.
    assert!(lockfile_path(tmp.path()).is_file());
}

// ---------------------------------------------------------------------------
// SourceKind serialization tests
// ---------------------------------------------------------------------------

#[test]
fn source_kind_serde_roundtrip() {
    let lockfile = Lockfile {
        package: vec![
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
        ],
    };

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
    // Simulate an old lockfile without source_kind field.
    let toml_str = r#"
[[package]]
name = "legacy"
repo = "https://github.com/org/legacy.git"
commit = "abc"
"#;
    let lockfile: Lockfile = toml::from_str(toml_str).unwrap();
    assert_eq!(lockfile.package[0].source_kind, SourceKind::Git);
}

// ---------------------------------------------------------------------------
// audit + Local source tests
// ---------------------------------------------------------------------------

#[test]
fn audit_skips_unknown_repo_for_local_source() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");

    // Create local source checkout in global store.
    let checkout = package_dir(&store, "local-skill", "local").unwrap();
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(checkout.join("SKILL.md"), "# Local Skill").unwrap();

    let lockfile = Lockfile {
        package: vec![LockedPackage {
            name: "local-skill".to_string(),
            repo: String::new(),
            commit: String::new(),
            version: None,
            source_kind: SourceKind::Local,
            requested_version: None,
            resolved_ref: None,
        }],
    };
    let lp = lockfile_path(tmp.path());
    save_lockfile(&lp, &lockfile).unwrap();

    let results = audit(tmp.path(), &store).unwrap();
    // No issues — empty repo is expected for Local sources.
    assert!(results.is_empty(), "expected no issues, got: {results:?}");
}

// ---------------------------------------------------------------------------
// lock preserves source_kind tests
// ---------------------------------------------------------------------------

#[test]
fn lock_preserves_source_kind_from_existing_lockfile() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");

    // Create local-source checkout in global store.
    let checkout = package_dir(&store, "local-dep", "local").unwrap();
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(checkout.join("SKILL.md"), "# Local").unwrap();

    // Create lockfile with Local source_kind.
    let initial = Lockfile {
        package: vec![LockedPackage {
            name: "local-dep".to_string(),
            repo: String::new(),
            commit: String::new(),
            version: None,
            source_kind: SourceKind::Local,
            requested_version: None,
            resolved_ref: None,
        }],
    };
    let lp = lockfile_path(tmp.path());
    save_lockfile(&lp, &initial).unwrap();

    // Re-lock — should preserve source_kind.
    let result = lock(tmp.path(), &store).unwrap();
    assert_eq!(result.package.len(), 1);
    assert_eq!(result.package[0].source_kind, SourceKind::Local);
}
