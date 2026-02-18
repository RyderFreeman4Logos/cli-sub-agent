//! Resolve a pattern by name from standard search paths.
//!
//! Patterns are higher-level constructs that embed skills inside a
//! `patterns/<name>/skills/<name>/` directory layout. This resolver
//! searches for that layout and returns the embedded skill content.
//!
//! Search order (first match wins):
//! 1. `.csa/patterns/<name>/`               (project-local fork)
//! 2. `patterns/<name>/`                    (repo-shipped patterns)
//! 3. `.weave/deps/*/patterns/<name>/`      (weave-installed packages)

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use tracing::debug;

use weave::parser::{AgentConfig, SkillConfig, parse_skill_config};

/// A pattern resolved from disk, with its embedded skill content.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedPattern {
    /// Root directory of the pattern (e.g. `patterns/csa-review/`).
    pub dir: PathBuf,
    /// Raw content of `skills/<name>/SKILL.md`.
    pub skill_md: String,
    /// Parsed `.skill.toml` configuration (if present at the pattern root).
    pub config: Option<SkillConfig>,
}

impl ResolvedPattern {
    /// Return the agent config section, if any.
    pub fn agent_config(&self) -> Option<&AgentConfig> {
        self.config.as_ref().and_then(|c| c.agent.as_ref())
    }
}

/// Resolve a pattern by name, searching standard paths in priority order.
///
/// `project_root` is the working directory / project root for the CSA run.
pub(crate) fn resolve_pattern(name: &str, project_root: &Path) -> Result<ResolvedPattern> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        bail!("Invalid pattern name: '{name}' (must be a simple name, no path separators)");
    }

    let candidates = search_paths(name, project_root);

    for dir in &candidates {
        let skill_md_path = dir.join("skills").join(name).join("SKILL.md");
        if skill_md_path.is_file() {
            let skill_md = std::fs::read_to_string(&skill_md_path)
                .with_context(|| format!("failed to read {}", skill_md_path.display()))?;

            let config = load_skill_config(dir)?;

            debug!(pattern_dir = %dir.display(), "Pattern resolved");

            return Ok(ResolvedPattern {
                dir: dir.clone(),
                skill_md,
                config,
            });
        }
    }

    bail!(
        "Pattern '{name}' not found. Searched:\n{}",
        candidates
            .iter()
            .map(|p| format!("  - {}/skills/{name}/SKILL.md", p.display()))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

/// Build the ordered list of directories to search for a pattern.
fn search_paths(name: &str, project_root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::with_capacity(4);

    // 1. Project-local fork: .csa/patterns/<name>/
    paths.push(project_root.join(".csa").join("patterns").join(name));

    // 2. Repo-shipped: patterns/<name>/
    paths.push(project_root.join("patterns").join(name));

    // 3. Weave-installed packages: .weave/deps/*/patterns/<name>/
    let weave_deps = project_root.join(".weave").join("deps");
    if let Ok(entries) = std::fs::read_dir(&weave_deps) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                paths.push(entry.path().join("patterns").join(name));
            }
        }
    }

    paths
}

/// Load `.skill.toml` from a pattern directory (optional file).
fn load_skill_config(pattern_dir: &Path) -> Result<Option<SkillConfig>> {
    let config_path = pattern_dir.join(".skill.toml");
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
    fn resolve_pattern_from_weave_deps() {
        let tmp = TempDir::new().unwrap();
        // Simulate a weave package: .weave/deps/some-pkg/patterns/csa-review/skills/csa-review/SKILL.md
        make_pattern_dir(
            tmp.path(),
            ".weave/deps/some-pkg/patterns/csa-review",
            "csa-review",
            "# CSA Review\nWeave-installed.",
            None,
        );

        let resolved = resolve_pattern("csa-review", tmp.path()).unwrap();
        assert!(resolved.skill_md.contains("Weave-installed"));
        assert!(
            resolved
                .dir
                .to_str()
                .unwrap()
                .contains(".weave/deps/some-pkg/patterns/csa-review")
        );
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
    fn resolve_pattern_repo_takes_priority_over_weave() {
        let tmp = TempDir::new().unwrap();
        make_pattern_dir(
            tmp.path(),
            "patterns/debate",
            "debate",
            "# Repo Debate",
            None,
        );
        make_pattern_dir(
            tmp.path(),
            ".weave/deps/pkg/patterns/debate",
            "debate",
            "# Weave Debate",
            None,
        );

        let resolved = resolve_pattern("debate", tmp.path()).unwrap();
        assert!(resolved.skill_md.contains("Repo Debate"));
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
}
