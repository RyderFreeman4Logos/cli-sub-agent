//! Tests for install_from_local and version pinning lockfile behavior.
//!
//! Split from `package_tests.rs` to stay under the monolith-file limit.

use tempfile::TempDir;

use super::*;

// ---------------------------------------------------------------------------
// install_from_local tests
// ---------------------------------------------------------------------------

#[test]
fn install_from_local_detects_case_mismatch_skill_md() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    // Create a skill directory with lowercase `skill.md`.
    let skill_src = tmp.path().join("bad-case-skill");
    std::fs::create_dir_all(&skill_src).unwrap();
    std::fs::write(skill_src.join("skill.md"), "# Wrong Case").unwrap();

    // Skip on case-insensitive filesystems (e.g. macOS HFS+/APFS default).
    // On such systems `SKILL.md` resolves to the same inode as `skill.md`,
    // so the detection logic in `install_from_local` cannot trigger the
    // case-mismatch error path.
    let probe = skill_src.join("_CaSe_PrObE_");
    std::fs::write(&probe, "").unwrap();
    if skill_src.join("_case_probe_").exists() {
        std::fs::remove_file(&probe).unwrap();
        // Case-insensitive FS: detection cannot work, skip test.
        return;
    }
    std::fs::remove_file(&probe).unwrap();

    let result = install_from_local(&skill_src, &project);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("skill.md") && err.contains("SKILL.md") && err.contains("Rename"),
        "unhelpful error: {err}"
    );
}

#[test]
fn install_from_local_succeeds() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    // Create a local skill directory.
    let skill_src = tmp.path().join("my-skill");
    std::fs::create_dir_all(&skill_src).unwrap();
    std::fs::write(skill_src.join("SKILL.md"), "# My Skill").unwrap();
    std::fs::write(skill_src.join("helper.txt"), "data").unwrap();

    let pkg = install_from_local(&skill_src, &project).unwrap();
    assert_eq!(pkg.name, "my-skill");
    assert_eq!(pkg.source_kind, SourceKind::Local);
    assert!(pkg.repo.is_empty());
    assert!(pkg.commit.is_empty());

    // Files were copied to deps.
    let dest = project.join(".weave").join("deps").join("my-skill");
    assert!(dest.join("SKILL.md").is_file());
    assert!(dest.join("helper.txt").is_file());

    // Lockfile was written to new path.
    let lockfile = load_lockfile(&lockfile_path(&project)).unwrap();
    assert_eq!(lockfile.package.len(), 1);
    assert_eq!(lockfile.package[0].source_kind, SourceKind::Local);
}

#[test]
fn install_from_local_excludes_git_dir() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let skill_src = tmp.path().join("git-skill");
    std::fs::create_dir_all(skill_src.join(".git")).unwrap();
    std::fs::write(skill_src.join("SKILL.md"), "# Git Skill").unwrap();
    std::fs::write(skill_src.join(".git").join("config"), "core").unwrap();

    install_from_local(&skill_src, &project).unwrap();

    let dest = project.join(".weave").join("deps").join("git-skill");
    assert!(dest.join("SKILL.md").is_file());
    assert!(!dest.join(".git").exists());
}

#[test]
fn install_from_local_rejects_missing_skill_md() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let skill_src = tmp.path().join("no-skill");
    std::fs::create_dir_all(&skill_src).unwrap();
    std::fs::write(skill_src.join("README.md"), "# No Skill").unwrap();

    let result = install_from_local(&skill_src, &project);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("SKILL.md not found"), "error: {err}");
}

#[cfg(unix)]
#[test]
fn install_from_local_rejects_symlinked_skill_md() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let skill_src = tmp.path().join("symlink-skill");
    std::fs::create_dir_all(&skill_src).unwrap();
    // Create a real SKILL.md elsewhere and symlink to it.
    let real_skill = tmp.path().join("real-SKILL.md");
    std::fs::write(&real_skill, "# Real Skill").unwrap();
    std::os::unix::fs::symlink(&real_skill, skill_src.join("SKILL.md")).unwrap();

    let result = install_from_local(&skill_src, &project);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("symlink"), "error: {err}");
}

#[test]
fn install_from_local_rejects_self_overwrite() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    // Pre-populate .weave/deps/overlap-skill as if it was already installed.
    let dest = project.join(".weave").join("deps").join("overlap-skill");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("SKILL.md"), "# Overlap Skill").unwrap();

    // Try installing from the destination itself — must fail.
    let result = install_from_local(&dest, &project);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("overlap"), "error: {err}");

    // Original content must survive.
    assert!(dest.join("SKILL.md").is_file());
}

#[test]
fn install_from_local_rejects_overlap_when_dest_not_exists() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    // Source is inside project but dest (.weave/deps/project) does NOT exist yet.
    // This must still be caught: source contains the would-be destination.
    let skill_src = tmp.path().join("new-skill");
    std::fs::create_dir_all(&skill_src).unwrap();
    std::fs::write(skill_src.join("SKILL.md"), "# New Skill").unwrap();

    // Name the skill the same as its own dest to force overlap when dest
    // doesn't exist yet — craft source as .weave/deps/new-skill inside a
    // fresh project where .weave/deps doesn't exist.
    let nested_project = skill_src.clone(); // project root == skill_src
    let result = install_from_local(&skill_src, &nested_project);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("overlap"), "error: {err}");
}

#[cfg(unix)]
#[test]
fn install_from_local_rejects_overlap_through_weave_symlink() {
    use std::os::unix::fs::symlink;

    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    // Create a source skill outside the project.
    let skill_src = tmp.path().join("my-skill");
    std::fs::create_dir_all(&skill_src).unwrap();
    std::fs::write(skill_src.join("SKILL.md"), "# Skill").unwrap();

    // Make .weave a symlink pointing INTO the source skill directory.
    // Without the symlink-resolving fix, the overlap guard would miss this.
    let weave_target = skill_src.join("nested");
    std::fs::create_dir_all(&weave_target).unwrap();
    symlink(&weave_target, project.join(".weave")).unwrap();

    let result = install_from_local(&skill_src, &project);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("overlap"),
        "expected overlap error, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// Version pinning tests
// ---------------------------------------------------------------------------

#[test]
fn parse_source_version_specifiers() {
    // Tag-style version.
    let src = parse_source("user/repo@v1.2.0").unwrap();
    assert_eq!(src.url, "https://github.com/user/repo.git");
    assert_eq!(src.git_ref, Some("v1.2.0".to_string()));
    assert_eq!(src.name, "repo");

    // Branch name.
    let src = parse_source("user/repo@main").unwrap();
    assert_eq!(src.git_ref, Some("main".to_string()));

    // Commit hash (short).
    let src = parse_source("user/repo@abc123").unwrap();
    assert_eq!(src.git_ref, Some("abc123".to_string()));

    // Full URL with tag.
    let src = parse_source("https://github.com/org/tool@v3.0.0").unwrap();
    assert_eq!(src.url, "https://github.com/org/tool.git");
    assert_eq!(src.git_ref, Some("v3.0.0".to_string()));

    // Hash-style ref.
    let src = parse_source("user/repo#develop").unwrap();
    assert_eq!(src.git_ref, Some("develop".to_string()));
}

#[test]
fn lockfile_roundtrip_with_version_pinning() {
    let lockfile = Lockfile {
        package: vec![
            LockedPackage {
                name: "pinned".to_string(),
                repo: "https://github.com/org/pinned.git".to_string(),
                commit: "abc123def456".to_string(),
                version: Some("1.0.0".to_string()),
                source_kind: SourceKind::Git,
                requested_version: Some("v1.0.0".to_string()),
                resolved_ref: Some("v1.0.0".to_string()),
            },
            LockedPackage {
                name: "unpinned".to_string(),
                repo: "https://github.com/org/unpinned.git".to_string(),
                commit: "789abcdef".to_string(),
                version: None,
                source_kind: SourceKind::Git,
                requested_version: None,
                resolved_ref: None,
            },
            LockedPackage {
                name: "branch-pinned".to_string(),
                repo: "https://github.com/org/bp.git".to_string(),
                commit: "deadbeef".to_string(),
                version: None,
                source_kind: SourceKind::Git,
                requested_version: Some("main".to_string()),
                resolved_ref: Some("main".to_string()),
            },
        ],
    };

    let tmp = TempDir::new().unwrap();
    let lock_path = tmp.path().join("lock.toml");

    save_lockfile(&lock_path, &lockfile).unwrap();
    let loaded = load_lockfile(&lock_path).unwrap();
    assert_eq!(lockfile, loaded);

    // Verify pinned fields survive roundtrip.
    assert_eq!(
        loaded.package[0].requested_version,
        Some("v1.0.0".to_string())
    );
    assert_eq!(loaded.package[0].resolved_ref, Some("v1.0.0".to_string()));
    assert!(loaded.package[1].requested_version.is_none());
    assert!(loaded.package[1].resolved_ref.is_none());
    assert_eq!(
        loaded.package[2].requested_version,
        Some("main".to_string())
    );
}

#[test]
fn old_lockfile_without_version_fields_defaults_to_none() {
    // Simulate an old lockfile without requested_version / resolved_ref.
    let toml_str = r#"
[[package]]
name = "legacy-pkg"
repo = "https://github.com/org/legacy.git"
commit = "abc123"
version = "1.0"
"#;
    let lockfile: Lockfile = toml::from_str(toml_str).unwrap();
    assert!(lockfile.package[0].requested_version.is_none());
    assert!(lockfile.package[0].resolved_ref.is_none());
}

#[test]
fn version_fields_omitted_from_toml_when_none() {
    let lockfile = Lockfile {
        package: vec![LockedPackage {
            name: "no-pin".to_string(),
            repo: "https://github.com/org/no-pin.git".to_string(),
            commit: "abc".to_string(),
            version: None,
            source_kind: SourceKind::Git,
            requested_version: None,
            resolved_ref: None,
        }],
    };

    let serialized = toml::to_string_pretty(&lockfile).unwrap();
    assert!(
        !serialized.contains("requested_version"),
        "None fields should be omitted: {serialized}"
    );
    assert!(
        !serialized.contains("resolved_ref"),
        "None fields should be omitted: {serialized}"
    );
}

#[test]
fn version_fields_present_in_toml_when_set() {
    let lockfile = Lockfile {
        package: vec![LockedPackage {
            name: "pinned".to_string(),
            repo: "https://github.com/org/pinned.git".to_string(),
            commit: "abc".to_string(),
            version: None,
            source_kind: SourceKind::Git,
            requested_version: Some("v2.0".to_string()),
            resolved_ref: Some("v2.0".to_string()),
        }],
    };

    let serialized = toml::to_string_pretty(&lockfile).unwrap();
    assert!(
        serialized.contains("requested_version"),
        "expected requested_version in: {serialized}"
    );
    assert!(
        serialized.contains("resolved_ref"),
        "expected resolved_ref in: {serialized}"
    );
}

#[test]
fn lock_preserves_requested_version_from_existing_lockfile() {
    let tmp = TempDir::new().unwrap();

    // Create dep directory.
    let deps = tmp.path().join(".weave").join("deps").join("pinned-dep");
    std::fs::create_dir_all(&deps).unwrap();
    std::fs::write(deps.join("SKILL.md"), "# Pinned").unwrap();

    // Create lockfile with pinned version.
    let initial = Lockfile {
        package: vec![LockedPackage {
            name: "pinned-dep".to_string(),
            repo: "https://github.com/org/pinned.git".to_string(),
            commit: "abc123".to_string(),
            version: Some("1.0".to_string()),
            source_kind: SourceKind::Git,
            requested_version: Some("v1.0".to_string()),
            resolved_ref: Some("v1.0".to_string()),
        }],
    };
    let lp = lockfile_path(tmp.path());
    save_lockfile(&lp, &initial).unwrap();

    // Re-lock — should preserve requested_version and resolved_ref.
    let result = lock(tmp.path()).unwrap();
    assert_eq!(result.package.len(), 1);
    assert_eq!(
        result.package[0].requested_version,
        Some("v1.0".to_string())
    );
    assert_eq!(result.package[0].resolved_ref, Some("v1.0".to_string()));
}
