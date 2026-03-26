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

#[test]
fn default_skill_directories_split_linking_from_checking() {
    assert_eq!(
        DEFAULT_LINK_DIRS,
        &[".claude/skills", ".codex/skills", ".agents/skills"]
    );
    assert!(DEFAULT_CHECK_DIRS.contains(&".gemini/skills"));
    assert!(!DEFAULT_LINK_DIRS.contains(&".gemini/skills"));
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

#[cfg(unix)]
#[test]
fn clean_gemini_duplicate_symlinks_removes_weave_managed_duplicates() {
    let tmp = TempDir::new().unwrap();
    let agents_dir = tmp.path().join(".agents").join("skills");
    let gemini_dir = tmp.path().join(".gemini").join("skills");
    std::fs::create_dir_all(&agents_dir).unwrap();
    std::fs::create_dir_all(&gemini_dir).unwrap();

    let indirect_agents_skill = agents_dir.join("commit");
    std::fs::create_dir_all(&indirect_agents_skill).unwrap();

    let direct_agents_skill = agents_dir.join("review");
    std::fs::create_dir_all(&direct_agents_skill).unwrap();

    let duplicate_link = gemini_dir.join("commit");
    let relative_duplicate = pathdiff::diff_paths(&indirect_agents_skill, &gemini_dir).unwrap();
    make_symlink(&duplicate_link, &relative_duplicate);

    let source_skill = tmp
        .path()
        .join("patterns")
        .join("review")
        .join("skills")
        .join("review");
    std::fs::create_dir_all(&source_skill).unwrap();

    let direct_link = gemini_dir.join("review");
    let relative_direct = pathdiff::diff_paths(&source_skill, &gemini_dir).unwrap();
    make_symlink(&direct_link, &relative_direct);

    let foreign_target = tmp.path().join("external-skill");
    std::fs::create_dir_all(&foreign_target).unwrap();
    let foreign_link = gemini_dir.join("external");
    make_symlink(&foreign_link, &foreign_target);

    std::fs::write(gemini_dir.join("README.md"), "not a symlink").unwrap();

    let result = clean_gemini_duplicate_symlinks(tmp.path()).unwrap();

    assert!(!result.missing_dir);
    assert_eq!(result.removed.len(), 2);
    assert!(
        result
            .removed
            .iter()
            .any(|entry| entry.path == duplicate_link && entry.target == relative_duplicate)
    );
    assert!(
        result
            .removed
            .iter()
            .any(|entry| entry.path == direct_link && entry.target == relative_direct)
    );
    assert_eq!(result.skipped_non_duplicate, 1);
    assert_eq!(result.skipped_non_weave_target, 0);
    assert_eq!(result.skipped_non_symlink, 1);
    assert!(result.remove_failures.is_empty());
    assert!(duplicate_link.symlink_metadata().is_err());
    assert!(direct_link.symlink_metadata().is_err());
    assert!(
        foreign_link
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[cfg(unix)]
#[test]
fn clean_gemini_duplicate_symlinks_preserves_user_custom_override() {
    let tmp = TempDir::new().unwrap();
    let agents_dir = tmp.path().join(".agents").join("skills");
    let gemini_dir = tmp.path().join(".gemini").join("skills");
    std::fs::create_dir_all(&agents_dir).unwrap();
    std::fs::create_dir_all(&gemini_dir).unwrap();

    std::fs::create_dir_all(agents_dir.join("commit")).unwrap();

    let custom_target = tmp.path().join("custom-skills").join("commit");
    std::fs::create_dir_all(&custom_target).unwrap();

    let custom_link = gemini_dir.join("commit");
    make_symlink(&custom_link, &custom_target);

    let result = clean_gemini_duplicate_symlinks(tmp.path()).unwrap();

    assert!(!result.missing_dir);
    assert!(result.removed.is_empty());
    assert!(result.remove_failures.is_empty());
    assert_eq!(result.skipped_non_duplicate, 0);
    assert_eq!(result.skipped_non_weave_target, 1);
    assert!(
        custom_link
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[cfg(unix)]
#[test]
fn clean_gemini_duplicate_symlinks_removes_broken_duplicate() {
    let tmp = TempDir::new().unwrap();
    let agents_dir = tmp.path().join(".agents").join("skills");
    let gemini_dir = tmp.path().join(".gemini").join("skills");
    std::fs::create_dir_all(&agents_dir).unwrap();
    std::fs::create_dir_all(&gemini_dir).unwrap();

    std::fs::create_dir_all(agents_dir.join("commit")).unwrap();

    let broken_link = gemini_dir.join("commit");
    let broken_target = PathBuf::from("../../patterns/commit/skills/commit");
    make_symlink(&broken_link, &broken_target);

    let result = clean_gemini_duplicate_symlinks(tmp.path()).unwrap();

    assert!(!result.missing_dir);
    assert_eq!(result.removed.len(), 1);
    assert_eq!(result.removed[0].path, broken_link);
    assert_eq!(result.removed[0].target, broken_target);
    assert!(result.remove_failures.is_empty());
    assert_eq!(result.skipped_non_duplicate, 0);
    assert_eq!(result.skipped_non_weave_target, 0);
    assert!(gemini_dir.join("commit").symlink_metadata().is_err());
}

#[test]
fn clean_gemini_duplicate_symlinks_handles_missing_directory() {
    let tmp = TempDir::new().unwrap();

    let result = clean_gemini_duplicate_symlinks(tmp.path()).unwrap();

    assert!(result.missing_dir);
    assert_eq!(result.dir, tmp.path().join(".gemini").join("skills"));
    assert!(result.removed.is_empty());
    assert!(result.remove_failures.is_empty());
}

// ---------------------------------------------------------------------------
// migrate_gemini_skills tests
// ---------------------------------------------------------------------------

#[test]
fn migrate_gemini_skills_handles_missing_directory() {
    let tmp = TempDir::new().unwrap();

    let result = migrate_gemini_skills(tmp.path()).unwrap();

    assert!(result.missing_dir);
    assert!(result.removed.is_empty());
    assert!(result.moved.is_empty());
}

#[cfg(unix)]
#[test]
fn migrate_gemini_skills_moves_unique_skills_to_agents() {
    let tmp = TempDir::new().unwrap();
    let agents_dir = tmp.path().join(".agents").join("skills");
    let gemini_dir = tmp.path().join(".gemini").join("skills");
    std::fs::create_dir_all(&gemini_dir).unwrap();
    // .agents/skills/ does NOT exist yet — migration should create it.

    // Create a skill source in patterns/.
    let source_skill = tmp
        .path()
        .join("patterns")
        .join("commit")
        .join("skills")
        .join("commit");
    std::fs::create_dir_all(&source_skill).unwrap();

    // Symlink in .gemini/skills/ → patterns/commit/skills/commit.
    let gemini_link = gemini_dir.join("commit");
    let relative = pathdiff::diff_paths(&source_skill, &gemini_dir).unwrap();
    make_symlink(&gemini_link, &relative);

    let result = migrate_gemini_skills(tmp.path()).unwrap();

    assert!(!result.missing_dir);
    assert!(result.removed.is_empty(), "no duplicates to remove");
    assert_eq!(result.moved.len(), 1);
    assert_eq!(result.moved[0].gemini_path, gemini_link);
    assert_eq!(result.moved[0].agents_path, agents_dir.join("commit"));

    // Original removed, new one exists.
    assert!(gemini_link.symlink_metadata().is_err());
    assert!(
        agents_dir
            .join("commit")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
    // New symlink resolves to the correct target.
    let resolved = std::fs::canonicalize(agents_dir.join("commit")).unwrap();
    let expected = std::fs::canonicalize(&source_skill).unwrap();
    assert_eq!(resolved, expected);
}

#[cfg(unix)]
#[test]
fn migrate_gemini_skills_removes_duplicates_and_moves_unique() {
    let tmp = TempDir::new().unwrap();
    let agents_dir = tmp.path().join(".agents").join("skills");
    let gemini_dir = tmp.path().join(".gemini").join("skills");
    std::fs::create_dir_all(&agents_dir).unwrap();
    std::fs::create_dir_all(&gemini_dir).unwrap();

    // "commit" already in .agents/skills/ — duplicate.
    let agents_commit = agents_dir.join("commit");
    std::fs::create_dir_all(&agents_commit).unwrap();

    let commit_source = tmp
        .path()
        .join("patterns")
        .join("commit")
        .join("skills")
        .join("commit");
    std::fs::create_dir_all(&commit_source).unwrap();
    let gemini_commit = gemini_dir.join("commit");
    let rel_commit = pathdiff::diff_paths(&agents_commit, &gemini_dir).unwrap();
    make_symlink(&gemini_commit, &rel_commit);

    // "review" NOT in .agents/skills/ — should be moved.
    let review_source = tmp
        .path()
        .join("patterns")
        .join("review")
        .join("skills")
        .join("review");
    std::fs::create_dir_all(&review_source).unwrap();
    let gemini_review = gemini_dir.join("review");
    let rel_review = pathdiff::diff_paths(&review_source, &gemini_dir).unwrap();
    make_symlink(&gemini_review, &rel_review);

    // "external" points outside weave-managed paths — should be preserved.
    let external_target = tmp.path().join("custom").join("external");
    std::fs::create_dir_all(&external_target).unwrap();
    let gemini_external = gemini_dir.join("external");
    make_symlink(&gemini_external, &external_target);

    let result = migrate_gemini_skills(tmp.path()).unwrap();

    assert_eq!(result.removed.len(), 1, "one duplicate removed");
    assert_eq!(result.removed[0].path, gemini_commit);
    assert_eq!(result.moved.len(), 1, "one unique moved");
    assert_eq!(result.moved[0].gemini_path, gemini_review);
    assert_eq!(result.skipped_non_weave_target, 1, "external preserved");

    // Verify filesystem state.
    assert!(
        gemini_commit.symlink_metadata().is_err(),
        "duplicate removed"
    );
    assert!(
        gemini_review.symlink_metadata().is_err(),
        "moved original removed"
    );
    assert!(
        agents_dir
            .join("review")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink(),
        "moved to agents"
    );
    assert!(
        gemini_external
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink(),
        "external preserved"
    );
}

#[cfg(unix)]
#[test]
fn migrate_gemini_skills_handles_broken_symlinks() {
    let tmp = TempDir::new().unwrap();
    let agents_dir = tmp.path().join(".agents").join("skills");
    let gemini_dir = tmp.path().join(".gemini").join("skills");
    std::fs::create_dir_all(&gemini_dir).unwrap();
    // No .agents/skills/ yet.

    // Broken symlink pointing into patterns/ (weave-managed path pattern).
    let broken_link = gemini_dir.join("stale-skill");
    let broken_target = PathBuf::from("../../patterns/stale/skills/stale-skill");
    make_symlink(&broken_link, &broken_target);

    let result = migrate_gemini_skills(tmp.path()).unwrap();

    // Broken links with weave-managed targets should be moved (target re-created).
    // The new symlink will also be broken, but that's correct — weave link sync
    // will fix or remove it later.
    assert_eq!(result.moved.len(), 1);
    assert_eq!(result.moved[0].gemini_path, broken_link);
    assert!(broken_link.symlink_metadata().is_err(), "original removed");
    assert!(
        agents_dir
            .join("stale-skill")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink(),
        "migrated (even if target is broken)"
    );
}
