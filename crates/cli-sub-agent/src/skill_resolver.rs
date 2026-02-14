//! Resolve a skill by name from standard search paths.
//!
//! Search order:
//! 1. `./.csa/skills/<name>/`         (project-local)
//! 2. `~/.config/cli-sub-agent/skills/<name>/`  (global user)
//! 3. `.weave/deps/<name>/`           (weave-managed)

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

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
    let mut paths = Vec::with_capacity(3);

    // 1. Project-local: .csa/skills/<name>/
    paths.push(project_root.join(".csa").join("skills").join(name));

    // 2. Global user: ~/.config/cli-sub-agent/skills/<name>/
    if let Some(base) = directories::BaseDirs::new() {
        paths.push(
            base.config_dir()
                .join("cli-sub-agent")
                .join("skills")
                .join(name),
        );
    }

    // 3. Weave-managed: .weave/deps/<name>/
    paths.push(project_root.join(".weave").join("deps").join(name));

    paths
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
        let dir = base.join(rel);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), skill_md).unwrap();
        if let Some(toml_content) = skill_toml {
            fs::write(dir.join(".skill.toml"), toml_content).unwrap();
        }
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
    fn resolve_skill_from_weave_deps() {
        let tmp = TempDir::new().unwrap();
        // No .csa/skills, no global — only .weave/deps
        make_skill_dir(
            tmp.path(),
            ".weave/deps/audit",
            "# Audit Skill\nCheck things.",
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

        let resolved = resolve_skill("audit", tmp.path()).unwrap();
        assert!(resolved.skill_md.contains("Audit Skill"));
        let config = resolved.config.as_ref().unwrap();
        assert_eq!(config.skill.name, "audit");
        let agent = config.agent.as_ref().unwrap();
        assert_eq!(agent.tier.as_deref(), Some("tier1"));
        assert_eq!(agent.max_turns, Some(10));
        assert_eq!(agent.token_budget, Some(50000));
        assert_eq!(agent.skip_context, vec!["AGENTS.md"]);
        assert_eq!(agent.extra_context, vec!["rules/security.md"]);
        assert_eq!(agent.tools.len(), 1);
        assert_eq!(agent.tools[0].tool, "claude-code");
    }

    #[test]
    fn resolve_skill_csa_takes_priority_over_weave() {
        let tmp = TempDir::new().unwrap();
        make_skill_dir(tmp.path(), ".csa/skills/review", "# CSA Review", None);
        make_skill_dir(tmp.path(), ".weave/deps/review", "# Weave Review", None);

        let resolved = resolve_skill("review", tmp.path()).unwrap();
        assert!(resolved.skill_md.contains("CSA Review"));
    }

    #[test]
    fn resolve_skill_not_found() {
        let tmp = TempDir::new().unwrap();
        let result = resolve_skill("nonexistent", tmp.path());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not found"), "{err}");
        assert!(err.contains(".csa/skills/nonexistent"), "{err}");
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
