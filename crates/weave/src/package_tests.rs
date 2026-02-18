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
            source_kind: SourceKind::default(),
            requested_version: None,
            resolved_ref: None,
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
            source_kind: SourceKind::default(),
            requested_version: None,
            resolved_ref: None,
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
            source_kind: SourceKind::default(),
            requested_version: None,
            resolved_ref: None,
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
            source_kind: SourceKind::Git, // Git source with empty repo → UnknownRepo
            requested_version: None,
            resolved_ref: None,
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

// ---------------------------------------------------------------------------
// SKILL.md case-mismatch detection tests
// ---------------------------------------------------------------------------

#[test]
fn audit_detects_case_mismatch_skill_md() {
    let tmp = TempDir::new().unwrap();

    // Dep directory exists with lowercase `skill.md` instead of `SKILL.md`.
    let deps = tmp.path().join(".weave").join("deps").join("bad-case");
    std::fs::create_dir_all(&deps).unwrap();
    std::fs::write(deps.join("skill.md"), "# Wrong Case").unwrap();

    let lockfile = Lockfile {
        package: vec![LockedPackage {
            name: "bad-case".to_string(),
            repo: "https://example.com/bad-case.git".to_string(),
            commit: "abc".to_string(),
            version: None,
            source_kind: SourceKind::default(),
            requested_version: None,
            resolved_ref: None,
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

    // Dep directory has the correct `SKILL.md`.
    let deps = tmp.path().join(".weave").join("deps").join("good");
    std::fs::create_dir_all(&deps).unwrap();
    std::fs::write(deps.join("SKILL.md"), "# Good Skill").unwrap();

    let lockfile = Lockfile {
        package: vec![LockedPackage {
            name: "good".to_string(),
            repo: "https://example.com/good.git".to_string(),
            commit: "abc".to_string(),
            version: None,
            source_kind: SourceKind::default(),
            requested_version: None,
            resolved_ref: None,
        }],
    };
    let lock_path = tmp.path().join(".weave").join("lock.toml");
    save_lockfile(&lock_path, &lockfile).unwrap();

    let results = audit(tmp.path()).unwrap();
    assert!(results.is_empty(), "expected no issues, got: {results:?}");
}

#[test]
fn audit_neither_skill_md_variant_is_missing() {
    let tmp = TempDir::new().unwrap();

    // Dep directory exists but has NO skill.md variant at all.
    let deps = tmp.path().join(".weave").join("deps").join("empty");
    std::fs::create_dir_all(&deps).unwrap();

    let lockfile = Lockfile {
        package: vec![LockedPackage {
            name: "empty".to_string(),
            repo: "https://example.com/empty.git".to_string(),
            commit: "abc".to_string(),
            version: None,
            source_kind: SourceKind::default(),
            requested_version: None,
            resolved_ref: None,
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
            .any(|i| matches!(i, AuditIssue::MissingSkillMd)),
        "expected MissingSkillMd, got: {:?}",
        results[0].issues
    );
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

    let deps = tmp.path().join(".weave").join("deps").join("local-skill");
    std::fs::create_dir_all(&deps).unwrap();
    std::fs::write(deps.join("SKILL.md"), "# Local Skill").unwrap();

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
    let lock_path = tmp.path().join(".weave").join("lock.toml");
    save_lockfile(&lock_path, &lockfile).unwrap();

    let results = audit(tmp.path()).unwrap();
    // No issues — empty repo is expected for Local sources.
    assert!(results.is_empty(), "expected no issues, got: {results:?}");
}

// ---------------------------------------------------------------------------
// lock preserves source_kind tests
// ---------------------------------------------------------------------------

#[test]
fn lock_preserves_source_kind_from_existing_lockfile() {
    let tmp = TempDir::new().unwrap();

    // Create dep directory.
    let deps = tmp.path().join(".weave").join("deps").join("local-dep");
    std::fs::create_dir_all(&deps).unwrap();
    std::fs::write(deps.join("SKILL.md"), "# Local").unwrap();

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
    let lock_path = tmp.path().join(".weave").join("lock.toml");
    save_lockfile(&lock_path, &initial).unwrap();

    // Re-lock — should preserve source_kind.
    let result = lock(tmp.path()).unwrap();
    assert_eq!(result.package.len(), 1);
    assert_eq!(result.package[0].source_kind, SourceKind::Local);
}
