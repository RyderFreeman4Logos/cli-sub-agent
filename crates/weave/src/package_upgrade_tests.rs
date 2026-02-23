//! Tests for `upgrade()` — structured one-command upgrade for all packages.

use std::process::Command;
use tempfile::TempDir;

use super::*;

// ---------------------------------------------------------------------------
// Helper: create a local git bare repo, return (work_dir, bare_repo_path).
// Tests use the bare repo path directly as the `repo` field in LockedPackage,
// bypassing `install()` which tries to parse URLs.
// ---------------------------------------------------------------------------
fn setup_git_repo(tmp: &Path) -> (PathBuf, PathBuf) {
    fn run_git(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
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

    let work = tmp.join("work");
    let remote = tmp.join("remote.git");
    std::fs::create_dir_all(&work).unwrap();

    run_git(&work, &["init", "--quiet"]);
    run_git(&work, &["config", "user.email", "test@example.com"]);
    run_git(&work, &["config", "user.name", "Test"]);

    std::fs::write(work.join("SKILL.md"), "# v1\n").unwrap();
    std::fs::write(
        work.join(".skill.toml"),
        "[skill]\nname = \"test-skill\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    run_git(&work, &["add", "."]);
    run_git(&work, &["commit", "--quiet", "-m", "v1"]);
    run_git(&work, &["branch", "-M", "main"]);

    let status = Command::new("git")
        .args(["clone", "--bare", "--quiet"])
        .arg(&work)
        .arg(&remote)
        .status()
        .unwrap();
    assert!(status.success());

    run_git(
        &work,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );

    (work, remote)
}

/// Manually install a package from a bare repo into cache+store+lockfile,
/// without going through `install()` (which normalizes to https URLs).
fn manual_install(
    bare_repo: &Path,
    cache_root: &Path,
    store_root: &Path,
    project_root: &Path,
    name: &str,
    requested_version: Option<&str>,
) -> LockedPackage {
    let repo_str = bare_repo.to_str().unwrap().to_string();
    let cas = ensure_cached(cache_root, &repo_str).unwrap();
    let commit = resolve_commit(&cas, requested_version).unwrap();
    let dest = package_dir(store_root, name, &commit).unwrap();
    if !is_checkout_valid(&dest) {
        checkout_to(&cas, &commit, &dest).unwrap();
    }
    let version = read_version(&dest);
    let pkg = LockedPackage {
        name: name.to_string(),
        repo: repo_str,
        commit,
        version,
        source_kind: SourceKind::Git,
        requested_version: requested_version.map(|s| s.to_string()),
        resolved_ref: requested_version.map(|s| s.to_string()),
    };
    let lock_path = lockfile_path(project_root);
    let mut lockfile = load_lockfile(&lock_path).unwrap_or_default();
    upsert_package(&mut lockfile, &pkg);
    save_lockfile(&lock_path, &lockfile).unwrap();
    pkg
}

/// Push a new commit to the remote, returning the new commit hash.
fn push_new_version(work: &Path, version: &str) -> String {
    fn run_git(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    std::fs::write(
        work.join(".skill.toml"),
        format!("[skill]\nname = \"test-skill\"\nversion = \"{version}\"\n"),
    )
    .unwrap();
    std::fs::write(work.join("SKILL.md"), format!("# {version}\n")).unwrap();
    run_git(work, &["add", "."]);
    run_git(work, &["commit", "--quiet", "-m", &format!("{version}")]);
    let hash = run_git(work, &["rev-parse", "HEAD"]);
    run_git(work, &["push", "--quiet", "origin", "main"]);
    hash
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn upgrade_empty_lockfile() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let cache = tmp.path().join("cache");
    let store = tmp.path().join("store");

    let lf = Lockfile::default();
    save_lockfile(&lockfile_path(&project), &lf).unwrap();

    let results = upgrade(&project, &cache, &store, false).unwrap();
    assert!(results.is_empty());
}

#[test]
fn upgrade_skips_local_packages() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let cache = tmp.path().join("cache");
    let store = tmp.path().join("store");

    let lf = Lockfile::with_packages(vec![LockedPackage {
        name: "local-pkg".to_string(),
        repo: String::new(),
        commit: String::new(),
        version: Some("1.0".to_string()),
        source_kind: SourceKind::Local,
        requested_version: None,
        resolved_ref: None,
    }]);
    save_lockfile(&lockfile_path(&project), &lf).unwrap();

    let results = upgrade(&project, &cache, &store, false).unwrap();
    assert_eq!(results.len(), 1);
    assert!(
        matches!(&results[0].status, UpgradeStatus::Skipped { reason } if reason.contains("local")),
        "expected skipped for local, got: {:?}",
        results[0].status
    );
}

#[test]
fn upgrade_skips_pinned_unless_force() {
    let tmp = TempDir::new().unwrap();
    let (work, remote) = setup_git_repo(tmp.path());
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let cache = tmp.path().join("cache");
    let store = tmp.path().join("store");

    // Install with pin to "main".
    let pkg = manual_install(
        &remote,
        &cache,
        &store,
        &project,
        "test-skill",
        Some("main"),
    );
    let first_commit = pkg.commit.clone();

    // Push a new version.
    push_new_version(&work, "2.0.0");

    // Upgrade without --force: pinned package should be skipped.
    let results = upgrade(&project, &cache, &store, false).unwrap();
    assert_eq!(results.len(), 1);
    assert!(
        matches!(&results[0].status, UpgradeStatus::Skipped { reason } if reason.contains("pinned")),
        "expected pinned skip, got: {:?}",
        results[0].status
    );

    // Verify commit unchanged.
    let lf = load_lockfile(&lockfile_path(&project)).unwrap();
    assert_eq!(lf.package[0].commit, first_commit);

    // Upgrade with --force: should upgrade.
    let results = upgrade(&project, &cache, &store, true).unwrap();
    assert_eq!(results.len(), 1);
    assert!(
        matches!(&results[0].status, UpgradeStatus::Upgraded { .. }),
        "expected upgraded, got: {:?}",
        results[0].status
    );
}

#[test]
fn upgrade_reports_already_latest() {
    let tmp = TempDir::new().unwrap();
    let (_work, remote) = setup_git_repo(tmp.path());
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let cache = tmp.path().join("cache");
    let store = tmp.path().join("store");

    // Install latest (no pin).
    manual_install(&remote, &cache, &store, &project, "test-skill", None);

    // Upgrade: should report already latest.
    let results = upgrade(&project, &cache, &store, false).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, UpgradeStatus::AlreadyLatest);
}

#[test]
fn upgrade_upgrades_outdated_package() {
    let tmp = TempDir::new().unwrap();
    let (work, remote) = setup_git_repo(tmp.path());
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let cache = tmp.path().join("cache");
    let store = tmp.path().join("store");

    // Install v1.
    let pkg = manual_install(&remote, &cache, &store, &project, "test-skill", None);
    let old_commit = pkg.commit.clone();
    assert_eq!(pkg.version, Some("1.0.0".to_string()));

    // Push v2.
    let new_hash = push_new_version(&work, "2.0.0");

    // Upgrade.
    let results = upgrade(&project, &cache, &store, false).unwrap();
    assert_eq!(results.len(), 1);

    match &results[0].status {
        UpgradeStatus::Upgraded {
            old_commit: oc,
            old_version,
        } => {
            assert_eq!(oc, &old_commit);
            assert_eq!(old_version.as_deref(), Some("1.0.0"));
        }
        other => panic!("expected Upgraded, got: {other:?}"),
    }

    // Verify new state.
    assert_eq!(results[0].package.commit, new_hash);
    assert_eq!(results[0].package.version, Some("2.0.0".to_string()));

    // Verify lockfile was updated.
    let lf = load_lockfile(&lockfile_path(&project)).unwrap();
    assert_eq!(lf.package[0].commit, new_hash);
    assert_eq!(lf.package[0].version, Some("2.0.0".to_string()));
}

#[test]
fn upgrade_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let (work, remote) = setup_git_repo(tmp.path());
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let cache = tmp.path().join("cache");
    let store = tmp.path().join("store");

    // Install v1.
    manual_install(&remote, &cache, &store, &project, "test-skill", None);

    // Push v2.
    push_new_version(&work, "2.0.0");

    // First upgrade.
    let r1 = upgrade(&project, &cache, &store, false).unwrap();
    assert!(matches!(&r1[0].status, UpgradeStatus::Upgraded { .. }));

    // Second upgrade: should be already latest.
    let r2 = upgrade(&project, &cache, &store, false).unwrap();
    assert_eq!(r2[0].status, UpgradeStatus::AlreadyLatest);

    // Third upgrade: still already latest.
    let r3 = upgrade(&project, &cache, &store, false).unwrap();
    assert_eq!(r3[0].status, UpgradeStatus::AlreadyLatest);
}

#[test]
fn upgrade_mixed_packages() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let cache = tmp.path().join("cache");
    let store = tmp.path().join("store");

    let lf = Lockfile::with_packages(vec![
        LockedPackage {
            name: "local-only".to_string(),
            repo: String::new(),
            commit: String::new(),
            version: Some("1.0".to_string()),
            source_kind: SourceKind::Local,
            requested_version: None,
            resolved_ref: None,
        },
        LockedPackage {
            name: "no-repo".to_string(),
            repo: String::new(),
            commit: "abc123".to_string(),
            version: None,
            source_kind: SourceKind::Git,
            requested_version: None,
            resolved_ref: None,
        },
    ]);
    save_lockfile(&lockfile_path(&project), &lf).unwrap();

    let results = upgrade(&project, &cache, &store, false).unwrap();
    assert_eq!(results.len(), 2);

    // Local: skipped.
    assert!(matches!(
        &results[0].status,
        UpgradeStatus::Skipped { reason } if reason.contains("local")
    ));

    // No-repo: skipped.
    assert!(matches!(
        &results[1].status,
        UpgradeStatus::Skipped { reason } if reason.contains("no repository")
    ));
}

#[test]
fn upgrade_skips_empty_repo_packages() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let cache = tmp.path().join("cache");
    let store = tmp.path().join("store");

    let lf = Lockfile::with_packages(vec![LockedPackage {
        name: "orphan".to_string(),
        repo: String::new(),
        commit: "deadbeef".to_string(),
        version: None,
        source_kind: SourceKind::Git,
        requested_version: None,
        resolved_ref: None,
    }]);
    save_lockfile(&lockfile_path(&project), &lf).unwrap();

    let results = upgrade(&project, &cache, &store, false).unwrap();
    assert_eq!(results.len(), 1);
    assert!(matches!(&results[0].status, UpgradeStatus::Skipped { .. }));
}

#[test]
fn upgrade_preserves_lockfile_versions_section() {
    let tmp = TempDir::new().unwrap();
    let (_work, remote) = setup_git_repo(tmp.path());
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let cache = tmp.path().join("cache");
    let store = tmp.path().join("store");

    // Install package.
    manual_install(&remote, &cache, &store, &project, "test-skill", None);

    // Manually add versions section to lockfile.
    let mut lf = load_lockfile(&lockfile_path(&project)).unwrap();
    lf.versions = Some(
        toml::Value::try_from(toml::toml! {
            csa = "0.1.32"
            weave = "0.1.32"
        })
        .unwrap(),
    );
    save_lockfile(&lockfile_path(&project), &lf).unwrap();

    // Upgrade (no changes expected, but lockfile is re-saved).
    upgrade(&project, &cache, &store, false).unwrap();

    // Verify versions section preserved.
    let loaded = load_lockfile(&lockfile_path(&project)).unwrap();
    assert!(
        loaded.versions.is_some(),
        "versions section should be preserved after upgrade"
    );
}
