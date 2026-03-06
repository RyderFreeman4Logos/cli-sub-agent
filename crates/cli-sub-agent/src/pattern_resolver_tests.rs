use std::fs;
use std::path::Path;

use tempfile::TempDir;

use super::*;

/// Create a pattern directory with the standard layout:
/// `<base>/<rel>/skills/<skill_name>/SKILL.md` and optionally `.skill.toml`.
fn make_pattern_dir(
    base: &Path,
    rel: &str,
    skill_name: &str,
    skill_md: &str,
    skill_toml: Option<&str>,
) {
    let pattern_dir = base.join(rel);
    let skill_dir = pattern_dir.join("skills").join(skill_name);
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), skill_md).unwrap();
    if let Some(toml_content) = skill_toml {
        fs::write(pattern_dir.join(".skill.toml"), toml_content).unwrap();
    }
}

/// Write a minimal lockfile referencing a package in the global store.
fn write_lockfile(project_root: &Path, name: &str, commit: &str) {
    let content = format!(
        r#"[[package]]
name = "{name}"
repo = "https://github.com/test/{name}.git"
commit = "{commit}"
"#
    );
    fs::write(project_root.join("weave.lock"), content).unwrap();
}

/// Helper: write a TOML overlay file at the given path.
fn write_overlay(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

/// Normalize a path for assertions across platforms.
///
/// On macOS, temp directories may be reported as `/var/...` while canonical
/// resolution yields `/private/var/...`. We canonicalize the longest existing
/// prefix, then re-append the non-existing tail so logical paths compare equal.
fn normalize_path_for_compare(path: &Path) -> std::path::PathBuf {
    let mut existing_prefix = path.to_path_buf();
    let mut tail = Vec::new();
    while !existing_prefix.exists() {
        let Some(name) = existing_prefix.file_name() else {
            break;
        };
        tail.push(name.to_os_string());
        let Some(parent) = existing_prefix.parent() else {
            break;
        };
        existing_prefix = parent.to_path_buf();
    }

    let mut normalized = existing_prefix
        .canonicalize()
        .unwrap_or_else(|_| existing_prefix.clone());
    for segment in tail.iter().rev() {
        normalized.push(segment);
    }
    normalized
}

fn path_equivalent(lhs: &Path, rhs: &Path) -> bool {
    normalize_path_for_compare(lhs) == normalize_path_for_compare(rhs)
}

fn assert_paths_include(paths: &[std::path::PathBuf], expected: &Path, msg: &str) {
    assert!(
        paths
            .iter()
            .any(|candidate| path_equivalent(candidate, expected)),
        "{msg}. expected={}, candidates={paths:?}",
        expected.display()
    );
}

fn assert_paths_exclude(paths: &[std::path::PathBuf], expected: &Path, msg: &str) {
    assert!(
        !paths
            .iter()
            .any(|candidate| path_equivalent(candidate, expected)),
        "{msg}. forbidden={}, candidates={paths:?}",
        expected.display()
    );
}

// ------------------------------------------------------------------
// Resolution tests
// ------------------------------------------------------------------

#[test]
fn resolve_pattern_from_csa_patterns() {
    let tmp = TempDir::new().unwrap();
    make_pattern_dir(
        tmp.path(),
        ".csa/patterns/csa-review",
        "csa-review",
        "# CSA Review\nLocal fork.",
        None,
    );

    let resolved = resolve_pattern("csa-review", tmp.path()).unwrap();
    assert!(resolved.skill_md.contains("CSA Review"));
    assert!(resolved.config.is_none());
    assert!(resolved.dir.ends_with(".csa/patterns/csa-review"));
}

#[test]
fn resolve_pattern_from_repo_patterns() {
    let tmp = TempDir::new().unwrap();
    make_pattern_dir(
        tmp.path(),
        "patterns/debate",
        "debate",
        "# Debate\nRepo-shipped.",
        Some(
            r#"
[skill]
name = "debate"
version = "0.1.0"

[agent]
tier = "tier-2-standard"
max_turns = 30
tools = [{ tool = "auto" }]
"#,
        ),
    );

    let resolved = resolve_pattern("debate", tmp.path()).unwrap();
    assert!(resolved.skill_md.contains("Debate"));
    let config = resolved.config.as_ref().unwrap();
    assert_eq!(config.skill.name, "debate");
    let agent = config.agent.as_ref().unwrap();
    assert_eq!(agent.tier.as_deref(), Some("tier-2-standard"));
    assert_eq!(agent.max_turns, Some(30));
}

#[test]
fn resolve_pattern_from_global_store() {
    let tmp = TempDir::new().unwrap();
    let store = TempDir::new().unwrap();
    let commit = "abcdef1234567890";

    let pkg_dir = package::package_dir(store.path(), "some-pkg", commit).unwrap();
    make_pattern_dir(
        &pkg_dir,
        "patterns/csa-review",
        "csa-review",
        "# CSA Review\nGlobal store.",
        None,
    );

    write_lockfile(tmp.path(), "some-pkg", commit);

    let paths = search_paths_with_store("csa-review", tmp.path(), Some(store.path()));
    let found = paths.iter().find(|p| {
        p.join("skills")
            .join("csa-review")
            .join("SKILL.md")
            .is_file()
    });
    assert!(found.is_some(), "pattern not found in global store paths");
    let skill_md = fs::read_to_string(found.unwrap().join("skills/csa-review/SKILL.md")).unwrap();
    assert!(skill_md.contains("Global store"));
}

#[test]
fn resolve_pattern_csa_takes_priority_over_repo() {
    let tmp = TempDir::new().unwrap();
    make_pattern_dir(
        tmp.path(),
        ".csa/patterns/csa-review",
        "csa-review",
        "# CSA Local Fork",
        None,
    );
    make_pattern_dir(
        tmp.path(),
        "patterns/csa-review",
        "csa-review",
        "# Repo Shipped",
        None,
    );

    let resolved = resolve_pattern("csa-review", tmp.path()).unwrap();
    assert!(resolved.skill_md.contains("CSA Local Fork"));
}

#[test]
fn resolve_pattern_repo_takes_priority_over_global_store() {
    let tmp = TempDir::new().unwrap();
    let store = TempDir::new().unwrap();
    let commit = "abcdef1234567890";

    make_pattern_dir(
        tmp.path(),
        "patterns/debate",
        "debate",
        "# Repo Debate",
        None,
    );

    let pkg_dir = package::package_dir(store.path(), "pkg", commit).unwrap();
    make_pattern_dir(
        &pkg_dir,
        "patterns/debate",
        "debate",
        "# Global Store Debate",
        None,
    );
    write_lockfile(tmp.path(), "pkg", commit);

    let paths = search_paths_with_store("debate", tmp.path(), Some(store.path()));
    let first_match = paths
        .iter()
        .find(|p| p.join("skills").join("debate").join("SKILL.md").is_file());
    assert!(first_match.is_some());
    let content = fs::read_to_string(first_match.unwrap().join("skills/debate/SKILL.md")).unwrap();
    assert!(content.contains("Repo Debate"));
}

#[test]
fn search_paths_include_superproject_roots_for_submodule_project_root() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir(tmp.path().join(".git")).unwrap();
    fs::create_dir_all(
        tmp.path()
            .join(".git")
            .join("modules")
            .join("demo-submodule"),
    )
    .unwrap();
    let submodule_root = tmp.path().join("crates").join("demo-submodule");
    fs::create_dir_all(&submodule_root).unwrap();
    fs::write(
        submodule_root.join(".git"),
        "gitdir: ../../.git/modules/demo-submodule\n",
    )
    .unwrap();

    let paths = search_paths_with_store("csa-review", &submodule_root, None);
    assert_paths_include(
        &paths,
        &tmp.path().join(".csa").join("patterns").join("csa-review"),
        "expected superproject .csa/patterns path in resolver candidates",
    );
    assert_paths_include(
        &paths,
        &tmp.path().join("patterns").join("csa-review"),
        "expected superproject patterns/ path in resolver candidates",
    );
}

#[test]
fn search_paths_include_immediate_parent_for_nested_submodule_project_root() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir(tmp.path().join(".git")).unwrap();
    fs::create_dir_all(
        tmp.path()
            .join(".git")
            .join("modules")
            .join("outer")
            .join("modules")
            .join("inner"),
    )
    .unwrap();
    let inner_root = tmp.path().join("outer").join("inner");
    fs::create_dir_all(&inner_root).unwrap();
    fs::write(
        inner_root.join(".git"),
        "gitdir: ../../.git/modules/outer/modules/inner\n",
    )
    .unwrap();

    let paths = search_paths_with_store("csa-review", &inner_root, None);
    assert_paths_include(
        &paths,
        &tmp.path()
            .join("outer")
            .join(".csa")
            .join("patterns")
            .join("csa-review"),
        "expected immediate parent submodule .csa/patterns path in resolver candidates",
    );
    assert_paths_include(
        &paths,
        &tmp.path().join("outer").join("patterns").join("csa-review"),
        "expected immediate parent submodule patterns path in resolver candidates",
    );
    assert_paths_exclude(
        &paths,
        &tmp.path().join(".csa").join("patterns").join("csa-review"),
        "must not skip immediate parent and jump straight to top-level root for nested submodule layout",
    );
}

#[test]
fn search_paths_include_superproject_roots_for_worktree_submodule_project_root() {
    let tmp = TempDir::new().unwrap();
    let main_root = tmp.path().join("main-repo");
    let worktree_root = tmp.path().join("main-wt");
    fs::create_dir_all(main_root.join(".git")).unwrap();
    fs::create_dir_all(&worktree_root).unwrap();
    fs::create_dir_all(
        main_root
            .join(".git")
            .join("worktrees")
            .join("parent-wt")
            .join("modules")
            .join("demo-submodule"),
    )
    .unwrap();
    fs::write(
        main_root.join(".git/worktrees/parent-wt/gitdir"),
        format!("{}\n", worktree_root.join(".git").display()),
    )
    .unwrap();
    let submodule_root = worktree_root.join("crates").join("demo-submodule");
    fs::create_dir_all(&submodule_root).unwrap();
    fs::write(
        submodule_root.join(".git"),
        format!(
            "gitdir: {}\n",
            main_root
                .join(".git/worktrees/parent-wt/modules/demo-submodule")
                .display()
        ),
    )
    .unwrap();

    let paths = search_paths_with_store("csa-review", &submodule_root, None);
    assert_paths_include(
        &paths,
        &worktree_root
            .join(".csa")
            .join("patterns")
            .join("csa-review"),
        "expected superproject .csa/patterns path in resolver candidates for worktree layout",
    );
    assert_paths_include(
        &paths,
        &worktree_root.join("patterns").join("csa-review"),
        "expected superproject patterns path in resolver candidates for worktree layout",
    );
    assert_paths_exclude(
        &paths,
        &main_root.join(".csa").join("patterns").join("csa-review"),
        "must not fall back to main repository root for worktree submodule layout",
    );
}

#[test]
fn search_paths_do_not_include_main_root_for_plain_worktree_project_root() {
    let tmp = TempDir::new().unwrap();
    let main_root = tmp.path().join("main-repo");
    let worktree_root = tmp.path().join("main-wt");
    fs::create_dir_all(main_root.join(".git").join("worktrees").join("parent-wt")).unwrap();
    fs::create_dir_all(&worktree_root).unwrap();
    fs::write(
        worktree_root.join(".git"),
        format!(
            "gitdir: {}\n",
            main_root.join(".git/worktrees/parent-wt").display()
        ),
    )
    .unwrap();

    let paths = search_paths_with_store("csa-review", &worktree_root, None);
    assert_paths_include(
        &paths,
        &worktree_root
            .join(".csa")
            .join("patterns")
            .join("csa-review"),
        "expected current worktree root in resolver candidates",
    );
    assert_paths_exclude(
        &paths,
        &main_root.join(".csa").join("patterns").join("csa-review"),
        "plain linked worktree must not be treated as submodule lookup context",
    );
}

#[test]
fn resolve_pattern_falls_back_to_pattern_md() {
    let tmp = TempDir::new().unwrap();
    // Legacy layout: only PATTERN.md at pattern root, no skills/ directory.
    let pattern_dir = tmp.path().join("patterns").join("debate");
    fs::create_dir_all(&pattern_dir).unwrap();
    fs::write(pattern_dir.join("PATTERN.md"), "# Debate\nLegacy layout.").unwrap();

    let resolved = resolve_pattern("debate", tmp.path()).unwrap();
    assert!(resolved.skill_md.contains("Legacy layout"));
    assert!(resolved.dir.ends_with("patterns/debate"));
}

#[test]
fn resolve_pattern_skill_md_takes_priority() {
    let tmp = TempDir::new().unwrap();
    // Create both: PATTERN.md at root AND skills/<name>/SKILL.md
    let pattern_dir = tmp.path().join("patterns").join("debate");
    fs::create_dir_all(&pattern_dir).unwrap();
    fs::write(pattern_dir.join("PATTERN.md"), "# Legacy content").unwrap();

    let skill_dir = pattern_dir.join("skills").join("debate");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), "# New layout content").unwrap();

    let resolved = resolve_pattern("debate", tmp.path()).unwrap();
    assert!(
        resolved.skill_md.contains("New layout content"),
        "SKILL.md should take priority over PATTERN.md, got: {}",
        resolved.skill_md
    );
}

#[test]
fn resolve_pattern_not_found() {
    let tmp = TempDir::new().unwrap();
    let result = resolve_pattern("nonexistent", tmp.path());
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("not found"), "{err}");
    assert!(err.contains("patterns/nonexistent"), "{err}");
}

#[test]
fn resolve_pattern_invalid_name_rejected() {
    let tmp = TempDir::new().unwrap();
    assert!(resolve_pattern("", tmp.path()).is_err());
    assert!(resolve_pattern("../escape", tmp.path()).is_err());
    assert!(resolve_pattern("foo/bar", tmp.path()).is_err());
}

#[test]
fn resolve_pattern_parses_skill_toml() {
    let tmp = TempDir::new().unwrap();
    make_pattern_dir(
        tmp.path(),
        "patterns/csa-review",
        "csa-review",
        "# Review",
        Some(
            r#"
[skill]
name = "csa-review"
version = "0.1.0"

[agent]
tier = "tier-2-standard"
max_turns = 25
token_budget = 80000
skip_context = ["AGENTS.md"]
extra_context = ["rules/review.md"]

[[agent.tools]]
tool = "claude-code"

[[agent.tools]]
tool = "codex"
"#,
        ),
    );

    let resolved = resolve_pattern("csa-review", tmp.path()).unwrap();
    let agent = resolved.agent_config().unwrap();
    assert_eq!(agent.tier.as_deref(), Some("tier-2-standard"));
    assert_eq!(agent.max_turns, Some(25));
    assert_eq!(agent.token_budget, Some(80000));
    assert_eq!(agent.skip_context, vec!["AGENTS.md"]);
    assert_eq!(agent.extra_context, vec!["rules/review.md"]);
    assert_eq!(agent.tools.len(), 2);
}

#[test]
fn resolve_pattern_without_skill_toml() {
    let tmp = TempDir::new().unwrap();
    make_pattern_dir(
        tmp.path(),
        "patterns/simple",
        "simple",
        "# Simple Pattern",
        None,
    );

    let resolved = resolve_pattern("simple", tmp.path()).unwrap();
    assert!(resolved.config.is_none());
    assert!(resolved.agent_config().is_none());
}

// ------------------------------------------------------------------
// TOML merge utility tests
// ------------------------------------------------------------------

#[test]
fn merge_toml_tables_overlay_adds_keys() {
    let mut base: toml::value::Table = toml::from_str(
        r#"
[skill]
name = "foo"
"#,
    )
    .unwrap();
    let overlay: toml::value::Table = toml::from_str(
        r#"
[agent]
tier = "tier-1"
"#,
    )
    .unwrap();
    merge_toml_tables(&mut base, overlay);
    assert!(base.contains_key("agent"));
    assert_eq!(base["skill"]["name"].as_str(), Some("foo"));
}

#[test]
fn merge_toml_tables_overlay_overrides_scalar() {
    let mut base: toml::value::Table = toml::from_str(
        r#"
[agent]
max_turns = 10
tier = "tier-2"
"#,
    )
    .unwrap();
    let overlay: toml::value::Table = toml::from_str(
        r#"
[agent]
max_turns = 50
"#,
    )
    .unwrap();
    merge_toml_tables(&mut base, overlay);
    assert_eq!(base["agent"]["max_turns"].as_integer(), Some(50));
    assert_eq!(base["agent"]["tier"].as_str(), Some("tier-2"));
}

#[test]
fn merge_toml_tables_nested_deep_merge() {
    let mut base: toml::value::Table = toml::from_str(
        r#"
[skill]
name = "demo"
version = "1.0"

[agent]
tier = "tier-2"
max_turns = 10
"#,
    )
    .unwrap();
    let overlay: toml::value::Table = toml::from_str(
        r#"
[agent]
max_turns = 25
token_budget = 80000
"#,
    )
    .unwrap();
    merge_toml_tables(&mut base, overlay);
    assert_eq!(base["skill"]["name"].as_str(), Some("demo"));
    assert_eq!(base["skill"]["version"].as_str(), Some("1.0"));
    assert_eq!(base["agent"]["tier"].as_str(), Some("tier-2"));
    assert_eq!(base["agent"]["max_turns"].as_integer(), Some(25));
    assert_eq!(base["agent"]["token_budget"].as_integer(), Some(80000));
}

// ------------------------------------------------------------------
// Config cascade tests
// ------------------------------------------------------------------

#[test]
fn config_cascade_project_overrides_user() {
    let tmp = TempDir::new().unwrap();
    let user_cfg = TempDir::new().unwrap();

    make_pattern_dir(
        tmp.path(),
        "patterns/review",
        "review",
        "# Review",
        Some(
            r#"
[skill]
name = "review"

[agent]
tier = "tier-2"
max_turns = 10
"#,
        ),
    );

    write_overlay(
        &user_cfg.path().join("patterns/review.toml"),
        r#"
[agent]
max_turns = 20
"#,
    );

    write_overlay(
        &tmp.path().join(".csa/patterns/review.toml"),
        r#"
[agent]
max_turns = 50
token_budget = 100000
"#,
    );

    let config = load_skill_config_with_user_dir(
        &tmp.path().join("patterns/review"),
        "review",
        tmp.path(),
        Some(user_cfg.path()),
    )
    .unwrap()
    .unwrap();

    let agent = config.agent.unwrap();
    assert_eq!(agent.max_turns, Some(50));
    assert_eq!(agent.token_budget, Some(100000));
    assert_eq!(agent.tier.as_deref(), Some("tier-2"));
}

#[test]
fn config_cascade_user_overrides_package() {
    let tmp = TempDir::new().unwrap();
    let user_cfg = TempDir::new().unwrap();

    make_pattern_dir(
        tmp.path(),
        "patterns/lint",
        "lint",
        "# Lint",
        Some(
            r#"
[skill]
name = "lint"

[agent]
tier = "tier-3"
max_turns = 5
"#,
        ),
    );

    write_overlay(
        &user_cfg.path().join("patterns/lint.toml"),
        r#"
[agent]
tier = "tier-1"
"#,
    );

    let config = load_skill_config_with_user_dir(
        &tmp.path().join("patterns/lint"),
        "lint",
        tmp.path(),
        Some(user_cfg.path()),
    )
    .unwrap()
    .unwrap();

    let agent = config.agent.unwrap();
    assert_eq!(agent.tier.as_deref(), Some("tier-1"));
    assert_eq!(agent.max_turns, Some(5));
}

#[path = "pattern_resolver_tests_tail.rs"]
mod tests_tail;
