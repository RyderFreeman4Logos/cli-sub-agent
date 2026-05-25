use std::fs;

use tempfile::TempDir;

use super::*;

fn make_skill_dir(base: &Path, rel: &str, skill_md: &str, skill_toml: Option<&str>) {
    let dir = if rel.is_empty() || rel == "." {
        base.to_path_buf()
    } else {
        base.join(rel)
    };
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("SKILL.md"), skill_md).unwrap();
    if let Some(toml_content) = skill_toml {
        fs::write(dir.join(".skill.toml"), toml_content).unwrap();
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

/// Normalize a path for assertions across platforms.
///
/// On macOS, temp directories may be reported as `/var/...` while
/// canonical paths resolve to `/private/var/...`. We canonicalize the
/// longest existing prefix and keep the non-existing tail unchanged.
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

#[test]
fn resolve_skill_from_csa_skills() {
    let tmp = TempDir::new().unwrap();
    make_skill_dir(
        tmp.path(),
        ".csa/skills/my-skill",
        "# My Skill\nDo things.",
        None,
    );

    let resolved = resolve_skill("my-skill", tmp.path()).unwrap();
    assert!(resolved.skill_md.contains("My Skill"));
    assert!(resolved.config.is_none());
    assert!(resolved.dir.ends_with(".csa/skills/my-skill"));
}

#[test]
fn resolve_skill_from_global_store() {
    let tmp = TempDir::new().unwrap();
    let store = TempDir::new().unwrap();
    let commit = "abcdef1234567890";

    // Create skill in global store at <store>/audit/<prefix>/
    let pkg_dir = package::package_dir(store.path(), "audit", commit).unwrap();
    make_skill_dir(
        &pkg_dir,
        ".",
        "# Audit Skill\nGlobal store.",
        Some(
            r#"
[skill]
name = "audit"
version = "1.0"

[agent]
tier = "tier1"
max_turns = 10
token_budget = 50000
skip_context = ["AGENTS.md"]
extra_context = ["rules/security.md"]

[[agent.tools]]
tool = "claude-code"
"#,
        ),
    );

    // Write lockfile referencing this package.
    write_lockfile(tmp.path(), "audit", commit);

    let paths = search_paths_with_store("audit", tmp.path(), Some(store.path()));
    let found = paths.iter().find(|p| p.join("SKILL.md").is_file());
    assert!(found.is_some(), "skill not found in global store paths");

    let skill_md = fs::read_to_string(found.unwrap().join("SKILL.md")).unwrap();
    assert!(skill_md.contains("Global store"));
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

    let paths = search_paths_with_store("dev2merge", &submodule_root, None);
    assert_paths_include(
        &paths,
        &tmp.path().join(".csa").join("skills").join("dev2merge"),
        "expected superproject .csa/skills path in resolver candidates",
    );
    assert_paths_include(
        &paths,
        &tmp.path().join(".claude").join("skills").join("dev2merge"),
        "expected superproject .claude/skills path in resolver candidates",
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

    let paths = search_paths_with_store("dev2merge", &inner_root, None);
    assert_paths_include(
        &paths,
        &tmp.path()
            .join("outer")
            .join(".csa")
            .join("skills")
            .join("dev2merge"),
        "expected immediate parent submodule .csa/skills path in resolver candidates",
    );
    assert_paths_include(
        &paths,
        &tmp.path()
            .join("outer")
            .join(".claude")
            .join("skills")
            .join("dev2merge"),
        "expected immediate parent submodule .claude/skills path in resolver candidates",
    );
    assert_paths_exclude(
        &paths,
        &tmp.path().join(".csa").join("skills").join("dev2merge"),
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

    let paths = search_paths_with_store("dev2merge", &submodule_root, None);
    assert_paths_include(
        &paths,
        &worktree_root.join(".csa").join("skills").join("dev2merge"),
        "expected superproject .csa/skills path in resolver candidates for worktree layout",
    );
    assert_paths_include(
        &paths,
        &worktree_root
            .join(".claude")
            .join("skills")
            .join("dev2merge"),
        "expected superproject .claude/skills path in resolver candidates for worktree layout",
    );
    assert_paths_exclude(
        &paths,
        &main_root.join(".csa").join("skills").join("dev2merge"),
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

    let paths = search_paths_with_store("dev2merge", &worktree_root, None);
    assert_paths_include(
        &paths,
        &worktree_root.join(".csa").join("skills").join("dev2merge"),
        "expected current worktree root in resolver candidates",
    );
    assert_paths_exclude(
        &paths,
        &main_root.join(".csa").join("skills").join("dev2merge"),
        "plain linked worktree must not be treated as submodule lookup context",
    );
}

#[test]
fn resolve_skill_csa_takes_priority_over_global_store() {
    let tmp = TempDir::new().unwrap();
    let store = TempDir::new().unwrap();
    let commit = "abcdef1234567890";

    make_skill_dir(tmp.path(), ".csa/skills/review", "# CSA Review", None);

    let pkg_dir = package::package_dir(store.path(), "review", commit).unwrap();
    make_skill_dir(&pkg_dir, ".", "# Global Store Review", None);
    write_lockfile(tmp.path(), "review", commit);

    let paths = search_paths_with_store("review", tmp.path(), Some(store.path()));
    let first_match = paths.iter().find(|p| p.join("SKILL.md").is_file());
    assert!(first_match.is_some());
    let content = fs::read_to_string(first_match.unwrap().join("SKILL.md")).unwrap();
    assert!(content.contains("CSA Review"));
}

#[test]
fn resolve_skill_from_claude_skills() {
    let tmp = TempDir::new().unwrap();
    make_skill_dir(
        tmp.path(),
        ".claude/skills/my-skill",
        "# Claude Skill\nFrom .claude/skills.",
        None,
    );

    let resolved = resolve_skill("my-skill", tmp.path()).unwrap();
    assert!(resolved.skill_md.contains("Claude Skill"));
    assert!(resolved.dir.ends_with(".claude/skills/my-skill"));
}

#[test]
fn resolve_skill_from_csa_namespace() {
    let tmp = TempDir::new().unwrap();
    make_skill_dir(
        tmp.path(),
        "skills/csa/code-health-agent",
        "# Code Health Agent\nFrom bundled CSA namespace.",
        None,
    );

    let resolved = resolve_skill("code-health-agent", tmp.path()).unwrap();
    assert!(resolved.skill_md.contains("Code Health Agent"));
    assert!(resolved.dir.ends_with("skills/csa/code-health-agent"));
}

#[test]
fn resolve_skill_csa_takes_priority_over_claude() {
    let tmp = TempDir::new().unwrap();
    make_skill_dir(tmp.path(), ".csa/skills/review", "# CSA Review", None);
    make_skill_dir(tmp.path(), ".claude/skills/review", "# Claude Review", None);

    let resolved = resolve_skill("review", tmp.path()).unwrap();
    assert!(
        resolved.skill_md.contains("CSA Review"),
        ".csa/skills/ should take priority over .claude/skills/"
    );
}

#[test]
fn resolve_skill_not_found() {
    let tmp = TempDir::new().unwrap();
    let result = resolve_skill("nonexistent", tmp.path());
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("not found"), "{err}");
    assert!(err.contains(".csa/skills/nonexistent"), "{err}");
    assert!(
        err.contains(".claude/skills/nonexistent"),
        "error should mention .claude/skills/ path: {err}"
    );
}

#[test]
fn resolve_skill_invalid_name_rejected() {
    let tmp = TempDir::new().unwrap();
    assert!(resolve_skill("", tmp.path()).is_err());
    assert!(resolve_skill("../escape", tmp.path()).is_err());
    assert!(resolve_skill("foo/bar", tmp.path()).is_err());
}

#[test]
fn resolve_skill_parses_agent_config() {
    let tmp = TempDir::new().unwrap();
    make_skill_dir(
        tmp.path(),
        ".csa/skills/test-skill",
        "# Test",
        Some(
            r#"
[skill]
name = "test-skill"

[agent]
tier = "tier2"
max_turns = 5
token_budget = 100000

[[agent.tools]]
tool = "codex"
model = "gpt-5.1"
thinking_budget = "high"

[[agent.tools]]
tool = "claude-code"
"#,
        ),
    );

    let resolved = resolve_skill("test-skill", tmp.path()).unwrap();
    let agent = resolved.agent_config().unwrap();
    assert_eq!(agent.tier.as_deref(), Some("tier2"));
    assert_eq!(agent.max_turns, Some(5));
    assert_eq!(agent.token_budget, Some(100000));
    assert_eq!(agent.tools.len(), 2);
    assert_eq!(agent.tools[0].tool, "codex");
    assert_eq!(agent.tools[0].model.as_deref(), Some("gpt-5.1"));
    assert_eq!(agent.tools[0].thinking_budget.as_deref(), Some("high"));
}

#[test]
fn resolve_skill_without_toml_sidecar() {
    let tmp = TempDir::new().unwrap();
    make_skill_dir(
        tmp.path(),
        ".csa/skills/simple",
        "# Simple Skill\nJust a prompt.",
        None,
    );

    let resolved = resolve_skill("simple", tmp.path()).unwrap();
    assert!(resolved.config.is_none());
    assert!(resolved.agent_config().is_none());
}
