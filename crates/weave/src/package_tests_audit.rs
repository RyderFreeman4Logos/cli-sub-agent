//! Audit-related tests for the `package` module.
//!
//! Split from `package_tests.rs` to stay under the monolith-file limit.

use tempfile::TempDir;

use super::*;

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
    let lockfile = Lockfile::with_packages(vec![LockedPackage {
        name: "ghost".to_string(),
        repo: "https://example.com/ghost.git".to_string(),
        commit: "abc12345".to_string(),
        version: None,
        source_kind: SourceKind::default(),
        requested_version: None,
        resolved_ref: None,
    }]);
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
    let checkout = package_dir(&store, "broken", "abc12345").unwrap();
    std::fs::create_dir_all(&checkout).unwrap();

    let lockfile = Lockfile::with_packages(vec![LockedPackage {
        name: "broken".to_string(),
        repo: "https://example.com/broken.git".to_string(),
        commit: "abc12345".to_string(),
        version: None,
        source_kind: SourceKind::default(),
        requested_version: None,
        resolved_ref: None,
    }]);
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

    let lockfile = Lockfile::with_packages(vec![LockedPackage {
        name: "local".to_string(),
        repo: String::new(),
        commit: "abc12345".to_string(),
        version: None,
        source_kind: SourceKind::Git, // Git source with empty repo → UnknownRepo
        requested_version: None,
        resolved_ref: None,
    }]);
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

#[test]
fn audit_detects_case_mismatch_skill_md() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");
    let checkout = package_dir(&store, "bad-case", "abc12345").unwrap();
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(checkout.join("skill.md"), "# Wrong Case").unwrap();
    let probe = checkout.join("_CaSe_PrObE_");
    std::fs::write(&probe, "").unwrap();
    if checkout.join("_case_probe_").exists() {
        std::fs::remove_file(&probe).unwrap();
        return;
    }
    std::fs::remove_file(&probe).unwrap();

    let lockfile = Lockfile::with_packages(vec![LockedPackage {
        name: "bad-case".to_string(),
        repo: "https://example.com/bad-case.git".to_string(),
        commit: "abc12345".to_string(),
        version: None,
        source_kind: SourceKind::default(),
        requested_version: None,
        resolved_ref: None,
    }]);
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
    let checkout = package_dir(&store, "good", "abc12345").unwrap();
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(checkout.join("SKILL.md"), "# Good Skill").unwrap();

    let lockfile = Lockfile::with_packages(vec![LockedPackage {
        name: "good".to_string(),
        repo: "https://example.com/good.git".to_string(),
        commit: "abc12345".to_string(),
        version: None,
        source_kind: SourceKind::default(),
        requested_version: None,
        resolved_ref: None,
    }]);
    let lp = lockfile_path(tmp.path());
    save_lockfile(&lp, &lockfile).unwrap();

    let results = audit(tmp.path(), &store).unwrap();
    assert!(results.is_empty(), "expected no issues, got: {results:?}");
}

#[test]
fn audit_neither_skill_md_variant_is_missing() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");
    let checkout = package_dir(&store, "empty", "abc12345").unwrap();
    std::fs::create_dir_all(&checkout).unwrap();

    let lockfile = Lockfile::with_packages(vec![LockedPackage {
        name: "empty".to_string(),
        repo: "https://example.com/empty.git".to_string(),
        commit: "abc12345".to_string(),
        version: None,
        source_kind: SourceKind::default(),
        requested_version: None,
        resolved_ref: None,
    }]);
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

#[test]
fn audit_skips_unknown_repo_for_local_source() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join("store");
    let checkout = package_dir(&store, "local-skill", "local").unwrap();
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(checkout.join("SKILL.md"), "# Local Skill").unwrap();

    let lockfile = Lockfile::with_packages(vec![LockedPackage {
        name: "local-skill".to_string(),
        repo: String::new(),
        commit: String::new(),
        version: None,
        source_kind: SourceKind::Local,
        requested_version: None,
        resolved_ref: None,
    }]);
    let lp = lockfile_path(tmp.path());
    save_lockfile(&lp, &lockfile).unwrap();

    let results = audit(tmp.path(), &store).unwrap();
    assert!(results.is_empty(), "expected no issues, got: {results:?}");
}
