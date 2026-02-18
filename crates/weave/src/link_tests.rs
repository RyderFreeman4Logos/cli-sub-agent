use std::path::Path;

use tempfile::tempdir;

use super::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a mock package store with a pattern companion skill.
fn setup_store_with_skill(
    store_root: &Path,
    pkg_name: &str,
    commit: &str,
    pattern_name: &str,
) -> std::path::PathBuf {
    let prefix = &commit[..commit.len().min(8)];
    let pkg_dir = store_root.join(pkg_name).join(prefix);
    let skill_dir = pkg_dir
        .join("patterns")
        .join(pattern_name)
        .join("skills")
        .join(pattern_name);
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "---\nname: test\n---\n# Test").unwrap();
    // Also create the top-level SKILL.md for the package itself.
    std::fs::write(pkg_dir.join("SKILL.md"), "---\nname: pkg\n---\n# Pkg").unwrap();
    pkg_dir
}

/// Create a minimal weave.lock file.
fn write_lockfile(project_root: &Path, entries: &[(&str, &str, &str)]) {
    let mut content = String::new();
    for (name, repo, commit) in entries {
        content.push_str(&format!(
            "[[package]]\nname = \"{name}\"\nrepo = \"{repo}\"\ncommit = \"{commit}\"\n\n"
        ));
    }
    std::fs::write(project_root.join("weave.lock"), &content).unwrap();
}

// ---------------------------------------------------------------------------
// Discovery tests
// ---------------------------------------------------------------------------

#[test]
fn discover_finds_companion_skills() {
    let tmp = tempdir().unwrap();
    let store_root = tmp.path().join("store");
    let project_root = tmp.path().join("project");
    std::fs::create_dir_all(&project_root).unwrap();

    let commit = "abcdef1234567890";
    setup_store_with_skill(&store_root, "test-pkg", commit, "my-pattern");
    write_lockfile(
        &project_root,
        &[("test-pkg", "https://github.com/x/y", commit)],
    );

    // We need to override the store root for testing.
    // Since discover_skills() calls global_store_root() internally,
    // we test via the lower-level discover_skills_in_patterns.
    let patterns_dir = store_root
        .join("test-pkg")
        .join(&commit[..8])
        .join("patterns");
    let mut skills = Vec::new();
    discover_skills_in_patterns(&patterns_dir, "test-pkg", &mut skills).unwrap();

    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "my-pattern");
    assert_eq!(skills[0].package_name, "test-pkg");
}

#[test]
fn discover_skips_pattern_without_companion() {
    let tmp = tempdir().unwrap();
    let patterns_dir = tmp.path().join("patterns");
    let pattern_dir = patterns_dir.join("orphan");
    std::fs::create_dir_all(&pattern_dir).unwrap();
    // Pattern exists but no skills/<name>/SKILL.md.
    std::fs::write(pattern_dir.join("PATTERN.md"), "# Orphan").unwrap();

    let mut skills = Vec::new();
    discover_skills_in_patterns(&patterns_dir, "pkg", &mut skills).unwrap();

    assert!(skills.is_empty());
}

// ---------------------------------------------------------------------------
// Conflict detection tests
// ---------------------------------------------------------------------------

#[test]
fn precheck_detects_conflict() {
    let skills = vec![
        DiscoveredSkill {
            name: "commit".to_string(),
            package_name: "pkg-a".to_string(),
            source_dir: "/a/commit".into(),
        },
        DiscoveredSkill {
            name: "commit".to_string(),
            package_name: "pkg-b".to_string(),
            source_dir: "/b/commit".into(),
        },
    ];

    let errors = precheck_conflicts(&skills);
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        &errors[0].reason,
        LinkErrorKind::Conflict {
            existing_package,
            new_package,
        } if existing_package == "pkg-a" && new_package == "pkg-b"
    ));
}

#[test]
fn precheck_no_conflict_same_package() {
    let skills = vec![
        DiscoveredSkill {
            name: "commit".to_string(),
            package_name: "pkg-a".to_string(),
            source_dir: "/a/commit".into(),
        },
        DiscoveredSkill {
            name: "commit".to_string(),
            package_name: "pkg-a".to_string(),
            source_dir: "/a/commit2".into(),
        },
    ];

    let errors = precheck_conflicts(&skills);
    assert!(errors.is_empty());
}

// ---------------------------------------------------------------------------
// Symlink creation tests
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn create_skill_link_creates_symlink() {
    let tmp = tempdir().unwrap();
    let target_dir = tmp.path().join(".claude").join("skills");
    std::fs::create_dir_all(&target_dir).unwrap();

    let source_dir = tmp
        .path()
        .join("store")
        .join("patterns")
        .join("mktd")
        .join("skills")
        .join("mktd");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(source_dir.join("SKILL.md"), "test").unwrap();

    let store_root = tmp.path().join("store");
    let link_path = target_dir.join("mktd");

    let skill = DiscoveredSkill {
        name: "mktd".to_string(),
        package_name: "test".to_string(),
        source_dir: source_dir.clone(),
    };

    let outcome = create_skill_link(
        &link_path,
        &source_dir,
        &target_dir,
        &store_root,
        &skill,
        false,
    )
    .unwrap();
    assert!(matches!(outcome, LinkOutcome::Created { .. }));
    assert!(link_path.exists());
    assert!(
        std::fs::symlink_metadata(&link_path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[cfg(unix)]
#[test]
fn create_skill_link_skips_existing_correct() {
    let tmp = tempdir().unwrap();
    let target_dir = tmp.path().join(".claude").join("skills");
    std::fs::create_dir_all(&target_dir).unwrap();

    let source_dir = tmp.path().join("store").join("skill");
    std::fs::create_dir_all(&source_dir).unwrap();

    let link_path = target_dir.join("test-skill");
    let relative = pathdiff::diff_paths(&source_dir, &target_dir).unwrap();
    std::os::unix::fs::symlink(&relative, &link_path).unwrap();

    let store_root = tmp.path().join("store");
    let skill = DiscoveredSkill {
        name: "test-skill".to_string(),
        package_name: "pkg".to_string(),
        source_dir: source_dir.clone(),
    };

    let outcome = create_skill_link(
        &link_path,
        &source_dir,
        &target_dir,
        &store_root,
        &skill,
        false,
    )
    .unwrap();
    assert!(matches!(outcome, LinkOutcome::Skipped { .. }));
}

#[cfg(unix)]
#[test]
fn create_skill_link_replaces_broken() {
    let tmp = tempdir().unwrap();
    let target_dir = tmp.path().join(".claude").join("skills");
    std::fs::create_dir_all(&target_dir).unwrap();

    let link_path = target_dir.join("broken");
    std::os::unix::fs::symlink("/nonexistent/path", &link_path).unwrap();

    let source_dir = tmp.path().join("store").join("skill");
    std::fs::create_dir_all(&source_dir).unwrap();

    let store_root = tmp.path().join("store");
    let skill = DiscoveredSkill {
        name: "broken".to_string(),
        package_name: "pkg".to_string(),
        source_dir: source_dir.clone(),
    };

    let outcome = create_skill_link(
        &link_path,
        &source_dir,
        &target_dir,
        &store_root,
        &skill,
        false,
    )
    .unwrap();
    assert!(matches!(outcome, LinkOutcome::Replaced { .. }));
    assert!(link_path.exists());
}

#[cfg(unix)]
#[test]
fn create_skill_link_errors_on_foreign_symlink() {
    let tmp = tempdir().unwrap();
    let target_dir = tmp.path().join(".claude").join("skills");
    std::fs::create_dir_all(&target_dir).unwrap();

    let foreign_target = tmp.path().join("foreign");
    std::fs::create_dir_all(&foreign_target).unwrap();

    let link_path = target_dir.join("conflict");
    std::os::unix::fs::symlink(&foreign_target, &link_path).unwrap();

    let source_dir = tmp.path().join("store").join("skill");
    std::fs::create_dir_all(&source_dir).unwrap();

    let store_root = tmp.path().join("store");
    let skill = DiscoveredSkill {
        name: "conflict".to_string(),
        package_name: "pkg".to_string(),
        source_dir: source_dir.clone(),
    };

    let result = create_skill_link(
        &link_path,
        &source_dir,
        &target_dir,
        &store_root,
        &skill,
        false,
    );
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err.reason, LinkErrorKind::ForeignSymlink { .. }));
}

#[cfg(unix)]
#[test]
fn create_skill_link_errors_on_non_symlink() {
    let tmp = tempdir().unwrap();
    let target_dir = tmp.path().join(".claude").join("skills");
    std::fs::create_dir_all(&target_dir).unwrap();

    let link_path = target_dir.join("regular");
    std::fs::create_dir_all(&link_path).unwrap(); // Regular directory.

    let source_dir = tmp.path().join("store").join("skill");
    std::fs::create_dir_all(&source_dir).unwrap();

    let store_root = tmp.path().join("store");
    let skill = DiscoveredSkill {
        name: "regular".to_string(),
        package_name: "pkg".to_string(),
        source_dir: source_dir.clone(),
    };

    let result = create_skill_link(
        &link_path,
        &source_dir,
        &target_dir,
        &store_root,
        &skill,
        false,
    );
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err.reason, LinkErrorKind::NotASymlink { .. }));
}

#[cfg(unix)]
#[test]
fn create_skill_link_force_overwrites_non_symlink() {
    let tmp = tempdir().unwrap();
    let target_dir = tmp.path().join(".claude").join("skills");
    std::fs::create_dir_all(&target_dir).unwrap();

    let link_path = target_dir.join("override");
    std::fs::create_dir_all(&link_path).unwrap(); // Regular directory.

    let source_dir = tmp.path().join("store").join("skill");
    std::fs::create_dir_all(&source_dir).unwrap();

    let store_root = tmp.path().join("store");
    let skill = DiscoveredSkill {
        name: "override".to_string(),
        package_name: "pkg".to_string(),
        source_dir: source_dir.clone(),
    };

    let outcome = create_skill_link(
        &link_path,
        &source_dir,
        &target_dir,
        &store_root,
        &skill,
        true,
    )
    .unwrap();
    assert!(matches!(outcome, LinkOutcome::Replaced { .. }));
    assert!(
        std::fs::symlink_metadata(&link_path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

// ---------------------------------------------------------------------------
// Stale link removal tests
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn is_stale_link_detects_stale() {
    let tmp = tempdir().unwrap();
    let store_root = tmp.path().join("store");
    let skills_dir = tmp.path().join("skills");
    std::fs::create_dir_all(&skills_dir).unwrap();

    // Create a source dir inside the store (simulates a weave-managed skill).
    let source = store_root.join("pkg").join("abc12345").join("skill-a");
    std::fs::create_dir_all(&source).unwrap();

    // Create a symlink that points into the store.
    let link_path = skills_dir.join("skill-a");
    let relative = pathdiff::diff_paths(&source, &skills_dir).unwrap();
    std::os::unix::fs::symlink(&relative, &link_path).unwrap();

    let empty_names: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let empty_dirs: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();

    // When the skill name is NOT in the set and source not in dirs, it IS stale.
    assert!(is_stale_link(
        &link_path,
        &store_root,
        &empty_names,
        &empty_dirs
    ));

    // When the skill name IS in the set, it should NOT be stale.
    let mut active_set = std::collections::HashSet::new();
    active_set.insert("skill-a");
    assert!(!is_stale_link(
        &link_path,
        &store_root,
        &active_set,
        &empty_dirs
    ));
}

#[cfg(unix)]
#[test]
fn is_stale_link_preserves_renamed_link() {
    let tmp = tempdir().unwrap();
    let store_root = tmp.path().join("store");
    let skills_dir = tmp.path().join("skills");
    std::fs::create_dir_all(&skills_dir).unwrap();

    // Create a source dir inside the store.
    let source = store_root.join("pkg").join("abc12345").join("commit");
    std::fs::create_dir_all(&source).unwrap();

    // Create a RENAMED symlink (user renamed "commit" to "my-commit").
    let link_path = skills_dir.join("my-commit");
    let relative = pathdiff::diff_paths(&source, &skills_dir).unwrap();
    std::os::unix::fs::symlink(&relative, &link_path).unwrap();

    let empty_names: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut source_dirs = std::collections::HashSet::new();
    source_dirs.insert(source.canonicalize().unwrap());

    // Name "my-commit" is not in skill_names, but target IS in source_dirs.
    // Should NOT be considered stale (renamed link is preserved).
    assert!(!is_stale_link(
        &link_path,
        &store_root,
        &empty_names,
        &source_dirs
    ));
}

#[cfg(unix)]
#[test]
fn is_stale_link_ignores_non_weave_symlink() {
    let tmp = tempdir().unwrap();
    let store_root = tmp.path().join("store");
    std::fs::create_dir_all(&store_root).unwrap();
    let skills_dir = tmp.path().join("skills");
    std::fs::create_dir_all(&skills_dir).unwrap();

    // Create a symlink pointing outside the store.
    let foreign = tmp.path().join("foreign");
    std::fs::create_dir_all(&foreign).unwrap();
    let link_path = skills_dir.join("foreign-skill");
    std::os::unix::fs::symlink(&foreign, &link_path).unwrap();

    let empty_names: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let empty_dirs: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    // Not weave-managed, so not stale.
    assert!(!is_stale_link(
        &link_path,
        &store_root,
        &empty_names,
        &empty_dirs
    ));
}

#[test]
fn is_stale_link_returns_false_for_non_symlink() {
    let tmp = tempdir().unwrap();
    let store_root = tmp.path().join("store");
    std::fs::create_dir_all(&store_root).unwrap();

    let regular_dir = tmp.path().join("regular");
    std::fs::create_dir_all(&regular_dir).unwrap();

    let empty_names: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let empty_dirs: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    assert!(!is_stale_link(
        &regular_dir,
        &store_root,
        &empty_names,
        &empty_dirs
    ));
}

// ---------------------------------------------------------------------------
// Helper function tests
// ---------------------------------------------------------------------------

#[test]
fn paths_equivalent_same_path() {
    let tmp = tempdir().unwrap();
    let a = tmp.path().join("a");
    std::fs::create_dir_all(&a).unwrap();
    assert!(paths_equivalent(&a, &a));
}

#[test]
fn paths_equivalent_nonexistent() {
    assert!(paths_equivalent(
        Path::new("/nonexistent/a"),
        Path::new("/nonexistent/a")
    ));
}

#[test]
fn is_weave_managed_recognizes_subpath() {
    let tmp = tempdir().unwrap();
    let store = tmp.path().join("store");
    let subpath = store.join("pkg").join("abc12345");
    std::fs::create_dir_all(&subpath).unwrap();
    assert!(is_weave_managed_path(&subpath, &store));
}

#[test]
fn is_weave_managed_rejects_outside() {
    let tmp = tempdir().unwrap();
    let store = tmp.path().join("store");
    std::fs::create_dir_all(&store).unwrap();
    let outside = tmp.path().join("other");
    std::fs::create_dir_all(&outside).unwrap();
    assert!(!is_weave_managed_path(&outside, &store));
}
