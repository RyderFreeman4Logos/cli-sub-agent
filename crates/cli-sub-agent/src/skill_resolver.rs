//! Resolve a skill by name from standard search paths.
//!
//! Search scope:
//! - Current project root
//! - Optional superproject root when current root is a git submodule
//!
//! Search order:
//! 1. `./.csa/skills/<name>/`                    (project-local, CSA-specific)
//! 2. `./.claude/skills/<name>/`                 (project-local, Claude Code compat)
//! 3. `<superproject>/.csa/skills/<name>/`       (submodule only)
//! 4. `<superproject>/.claude/skills/<name>/`    (submodule only)
//! 5. `~/.config/cli-sub-agent/skills/<name>/`   (global user)
//! 6. `<global_store>/<name>/<commit>/`          (weave global store via lockfiles)

use anyhow::{Context, Result, bail};
use csa_config::paths;
use std::path::{Path, PathBuf};

use weave::package::{self, SourceKind};
use weave::parser::{AgentConfig, SkillConfig, parse_skill_config};

/// A skill resolved from disk, ready for injection into a CSA run.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedSkill {
    /// Directory where the skill lives.
    pub dir: PathBuf,
    /// Raw content of SKILL.md.
    pub skill_md: String,
    /// Parsed `.skill.toml` configuration (if present).
    pub config: Option<SkillConfig>,
}

impl ResolvedSkill {
    /// Return the agent config section, if any.
    pub fn agent_config(&self) -> Option<&AgentConfig> {
        self.config.as_ref().and_then(|c| c.agent.as_ref())
    }
}

/// Resolve a skill by name, searching standard paths in priority order.
///
/// `project_root` is the working directory / project root for the CSA run.
pub(crate) fn resolve_skill(name: &str, project_root: &Path) -> Result<ResolvedSkill> {
    // Validate name: no path separators, no empty
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        bail!("Invalid skill name: '{name}' (must be a simple name, no path separators)");
    }

    let candidates = search_paths(name, project_root);

    for dir in &candidates {
        let skill_md_path = dir.join("SKILL.md");
        if skill_md_path.is_file() {
            let skill_md = std::fs::read_to_string(&skill_md_path)
                .with_context(|| format!("failed to read {}", skill_md_path.display()))?;

            let config = load_skill_config(dir)?;

            return Ok(ResolvedSkill {
                dir: dir.clone(),
                skill_md,
                config,
            });
        }
    }

    bail!(
        "Skill '{name}' not found. Searched:\n{}",
        candidates
            .iter()
            .map(|p| format!("  - {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

/// Build the ordered list of directories to search for a skill.
fn search_paths(name: &str, project_root: &Path) -> Vec<PathBuf> {
    search_paths_with_store(
        name,
        project_root,
        package::global_store_root().ok().as_deref(),
    )
}

/// Build search paths using an explicit store root (testable).
fn search_paths_with_store(
    name: &str,
    project_root: &Path,
    store_root: Option<&Path>,
) -> Vec<PathBuf> {
    let mut search_roots = Vec::with_capacity(8);
    let repo_roots = discover_repo_roots(project_root);

    // 1. Project-local and ancestor repo roots:
    //    .csa/skills/<name>/ then .claude/skills/<name>/ for each root.
    for root in &repo_roots {
        search_roots.push(root.join(".csa").join("skills").join(name));
        search_roots.push(root.join(".claude").join("skills").join(name));
    }

    // 3. Global user: ~/.config/cli-sub-agent/skills/<name>/ (legacy fallback supported)
    if let Some(config_dir) = paths::config_dir() {
        search_roots.push(config_dir.join("skills").join(name));
    }

    // 4. Weave global store: match locked packages by name.
    if let Some(store) = store_root {
        for root in &repo_roots {
            if let Some(lockfile_path) = package::find_lockfile(root)
                && let Ok(lockfile) = package::load_lockfile(&lockfile_path)
            {
                for pkg in &lockfile.package {
                    if pkg.name != name {
                        continue;
                    }
                    let commit_key = match pkg.source_kind {
                        SourceKind::Local => "local",
                        SourceKind::Git if pkg.commit.is_empty() => continue,
                        SourceKind::Git => &pkg.commit,
                    };
                    if let Ok(pkg_dir) = package::package_dir(store, &pkg.name, commit_key) {
                        search_roots.push(pkg_dir);
                    }
                }
            }
        }
    }

    search_roots
}

fn discover_repo_roots(project_root: &Path) -> Vec<PathBuf> {
    let mut roots = vec![project_root.to_path_buf()];
    if let Some(super_root) = discover_superproject_root(project_root)
        && super_root != project_root
    {
        roots.push(super_root);
    }
    roots
}

fn discover_superproject_root(project_root: &Path) -> Option<PathBuf> {
    let git_marker = project_root.join(".git");
    if !git_marker.is_file() {
        return None;
    }
    let marker = std::fs::read_to_string(git_marker).ok()?;
    let gitdir_raw = marker.trim().strip_prefix("gitdir:")?.trim();
    if gitdir_raw.is_empty() {
        return None;
    }

    let gitdir_path = Path::new(gitdir_raw);
    let resolved_gitdir = if gitdir_path.is_absolute() {
        gitdir_path.to_path_buf()
    } else {
        project_root.join(gitdir_path)
    };
    let normalized_gitdir = resolved_gitdir
        .canonicalize()
        .unwrap_or(resolved_gitdir.clone());

    superproject_root_from_gitdir_path(&normalized_gitdir)
}

fn superproject_root_from_gitdir_path(gitdir: &Path) -> Option<PathBuf> {
    let components: Vec<_> = gitdir.components().collect();
    let dotgit_index = components
        .iter()
        .position(|component| component.as_os_str() == std::ffi::OsStr::new(".git"))?;

    let mut root = PathBuf::new();
    for component in &components[..dotgit_index] {
        root.push(component.as_os_str());
    }
    if root.as_os_str().is_empty() {
        return None;
    }

    let marker = components.get(dotgit_index + 1)?.as_os_str();
    if marker == std::ffi::OsStr::new("modules") {
        let modules_positions: Vec<usize> = components
            .iter()
            .enumerate()
            .skip(dotgit_index + 1)
            .filter_map(|(idx, component)| {
                (component.as_os_str() == std::ffi::OsStr::new("modules")).then_some(idx)
            })
            .collect();
        if modules_positions.len() <= 1 {
            return Some(root);
        }

        let first_modules = modules_positions[0];
        let last_modules = *modules_positions.last()?;
        let mut parent_root = root.clone();
        for component in &components[(first_modules + 1)..last_modules] {
            if component.as_os_str() == std::ffi::OsStr::new("modules") {
                continue;
            }
            parent_root.push(component.as_os_str());
        }
        return Some(parent_root);
    }

    if marker != std::ffi::OsStr::new("worktrees") {
        return None;
    }

    // Worktree submodule layout:
    // <main>/.git/worktrees/<worktree>/modules/<submodule...>
    let worktree_name = components.get(dotgit_index + 2)?.as_os_str();
    if components.get(dotgit_index + 3)?.as_os_str() != std::ffi::OsStr::new("modules") {
        return None;
    }

    let worktree_admin = root.join(".git").join("worktrees").join(worktree_name);
    let worktree_gitdir = std::fs::read_to_string(worktree_admin.join("gitdir")).ok()?;
    let worktree_gitdir = worktree_gitdir.trim();
    if worktree_gitdir.is_empty() {
        return None;
    }
    let worktree_gitdir_path = Path::new(worktree_gitdir);
    let resolved_worktree_gitdir = if worktree_gitdir_path.is_absolute() {
        worktree_gitdir_path.to_path_buf()
    } else {
        worktree_admin.join(worktree_gitdir_path)
    };
    let normalized_worktree_gitdir = resolved_worktree_gitdir
        .canonicalize()
        .unwrap_or(resolved_worktree_gitdir);
    normalized_worktree_gitdir.parent().map(Path::to_path_buf)
}

/// Load `.skill.toml` from a skill directory (optional file).
fn load_skill_config(skill_dir: &Path) -> Result<Option<SkillConfig>> {
    let config_path = skill_dir.join(".skill.toml");
    if !config_path.is_file() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;

    let config = parse_skill_config(&content)
        .with_context(|| format!("failed to parse {}", config_path.display()))?;

    Ok(Some(config))
}

#[cfg(test)]
mod tests {
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
}
