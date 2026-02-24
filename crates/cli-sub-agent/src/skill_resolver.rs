//! Resolve a skill by name from standard search paths.
//!
//! Search order:
//! 1. `./.csa/skills/<name>/`                    (project-local, CSA-specific)
//! 2. `./.claude/skills/<name>/`                 (project-local, Claude Code compat)
//! 3. `~/.config/cli-sub-agent/skills/<name>/`   (global user)
//! 4. `<global_store>/<name>/<commit>/`           (weave global store)

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
    let mut search_roots = Vec::with_capacity(4);

    // 1. Project-local (CSA-specific): .csa/skills/<name>/
    search_roots.push(project_root.join(".csa").join("skills").join(name));

    // 2. Project-local (Claude Code compat): .claude/skills/<name>/
    search_roots.push(project_root.join(".claude").join("skills").join(name));

    // 3. Global user: ~/.config/cli-sub-agent/skills/<name>/ (legacy fallback supported)
    if let Some(config_dir) = paths::config_dir() {
        search_roots.push(config_dir.join("skills").join(name));
    }

    // 4. Weave global store: match locked packages by name.
    if let Some(store) = store_root {
        if let Some(lockfile_path) = package::find_lockfile(project_root) {
            if let Ok(lockfile) = package::load_lockfile(&lockfile_path) {
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
