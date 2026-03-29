use super::*;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_empty_removed_names_returns_empty() {
    let dir = tempdir().unwrap();
    let result = scan_stale_skill_references(dir.path(), &[]);
    assert!(result.is_empty());
}

#[test]
fn test_detects_skill_pattern_in_settings() {
    let dir = tempdir().unwrap();
    let claude_dir = dir.path().join(".claude");
    fs::create_dir_all(&claude_dir).unwrap();
    fs::write(
        claude_dir.join("settings.local.json"),
        r#"{
  "permissions": {
    "allow": [
      "Skill(old-skill)",
      "Skill(old-skill:sub-action)"
    ]
  }
}"#,
    )
    .unwrap();

    let result = scan_stale_skill_references(dir.path(), &["old-skill".to_string()]);
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].skill_name, "old-skill");
    assert_eq!(result[0].line, 4);
    assert_eq!(result[1].skill_name, "old-skill");
    assert_eq!(result[1].line, 5);
}

#[test]
fn test_ignores_bash_history_in_settings() {
    let dir = tempdir().unwrap();
    let claude_dir = dir.path().join(".claude");
    fs::create_dir_all(&claude_dir).unwrap();
    fs::write(
        claude_dir.join("settings.local.json"),
        r#"{
  "permissions": {
    "allow": [
      "Bash(git commit -m \"feat(old-skill): initial\")"
    ]
  }
}"#,
    )
    .unwrap();

    let result = scan_stale_skill_references(dir.path(), &["old-skill".to_string()]);
    assert!(result.is_empty(), "Bash history entries should be ignored");
}

#[test]
fn test_detects_slash_reference_in_markdown() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("CLAUDE.md"),
        "# Project\n\nUse /old-skill to run the pipeline.\n",
    )
    .unwrap();

    let result = scan_stale_skill_references(dir.path(), &["old-skill".to_string()]);
    assert!(!result.is_empty(), "should detect /old-skill reference");
    assert_eq!(result[0].skill_name, "old-skill");
    assert_eq!(result[0].line, 3);
}

#[test]
fn test_skips_missing_files() {
    let dir = tempdir().unwrap();
    // Empty dir — no files at all
    let result = scan_stale_skill_references(dir.path(), &["some-skill".to_string()]);
    assert!(result.is_empty());
}

#[test]
fn test_detects_reference_in_agents_md() {
    let dir = tempdir().unwrap();
    let rules_dir = dir.path().join(".claude/rules");
    fs::create_dir_all(&rules_dir).unwrap();
    fs::write(
        rules_dir.join("AGENTS.md"),
        "## Skills\n\nRun `/old-skill` to invoke.\n",
    )
    .unwrap();

    let result = scan_stale_skill_references(dir.path(), &["old-skill".to_string()]);
    assert!(!result.is_empty(), "should detect /old-skill in AGENTS.md");
    assert_eq!(result[0].skill_name, "old-skill");
    assert_eq!(result[0].line, 3);
}

#[test]
fn test_detects_reference_in_root_agents_md() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("AGENTS.md"),
        "# Rules\n\nRun /old-skill after commit.\n",
    )
    .unwrap();

    let result = scan_stale_skill_references(dir.path(), &["old-skill".to_string()]);
    assert!(
        !result.is_empty(),
        "should detect old-skill in root AGENTS.md"
    );
    assert_eq!(result[0].file, PathBuf::from("AGENTS.md"));
    assert_eq!(result[0].line, 3);
}

#[test]
fn test_no_false_positive_on_bare_common_word() {
    // Bare word "commit" should NOT match — only precise patterns like
    // /commit, `commit`, or Skill(commit) should be flagged.
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("CLAUDE.md"),
        "Run commit before pushing. Use ai-reviewed-commit too.\n",
    )
    .unwrap();

    let result = scan_stale_skill_references(dir.path(), &["commit".to_string()]);
    assert!(
        result.is_empty(),
        "bare word 'commit' should NOT be flagged in markdown"
    );
}

#[test]
fn test_detects_backtick_quoted_reference() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("CLAUDE.md"),
        "Use `old-skill` for the pipeline.\n",
    )
    .unwrap();

    let result = scan_stale_skill_references(dir.path(), &["old-skill".to_string()]);
    assert!(
        !result.is_empty(),
        "should detect backtick-quoted `old-skill`"
    );
}

#[test]
fn test_detects_skill_pattern_in_markdown() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("CLAUDE.md"),
        "Allowed: Skill(old-skill) and Skill(old-skill:sub).\n",
    )
    .unwrap();

    let result = scan_stale_skill_references(dir.path(), &["old-skill".to_string()]);
    assert!(
        !result.is_empty(),
        "should detect Skill(old-skill) in markdown"
    );
}

#[test]
fn test_settings_skill_not_masked_by_bash_on_same_line() {
    // In minified JSON, Bash() and Skill() entries could be on the same line.
    // The Skill() entry should still be detected.
    let dir = tempdir().unwrap();
    let claude_dir = dir.path().join(".claude");
    fs::create_dir_all(&claude_dir).unwrap();
    fs::write(
        claude_dir.join("settings.local.json"),
        r#"{"allow":["Bash(git commit)","Skill(old-skill)"]}"#,
    )
    .unwrap();

    let result = scan_stale_skill_references(dir.path(), &["old-skill".to_string()]);
    assert!(
        !result.is_empty(),
        "Skill() should be detected even when Bash() is on the same line"
    );
}
