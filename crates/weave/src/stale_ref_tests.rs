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
        "## Skills\n\n**old-skill** — does something useful\n",
    )
    .unwrap();

    let result = scan_stale_skill_references(dir.path(), &["old-skill".to_string()]);
    assert!(!result.is_empty(), "should detect old-skill in AGENTS.md");
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
fn test_no_false_positive_on_hyphenated_superset() {
    // "commit" should NOT match inside "ai-reviewed-commit" because
    // hyphen is treated as a word character in skill names.
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("CLAUDE.md"),
        "Use ai-reviewed-commit for the pipeline.\n",
    )
    .unwrap();

    let result = scan_stale_skill_references(dir.path(), &["commit".to_string()]);
    assert!(
        result.is_empty(),
        "should NOT flag 'commit' inside 'ai-reviewed-commit'"
    );
}
