//! Resolve a pattern by name from standard search paths.
//!
//! Patterns are higher-level constructs that embed skills inside a
//! `patterns/<name>/skills/<name>/` directory layout. This resolver
//! searches for that layout and returns the embedded skill content.
//!
//! Search order (first match wins):
//! 1. `.csa/patterns/<name>/`               (project-local fork)
//! 2. `patterns/<name>/`                    (repo-shipped patterns)
//! 3. `<global_store>/<pkg>/<commit>/patterns/<name>/`  (weave global store)

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use tracing::debug;

use weave::package::{self, SourceKind};
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
    let mut paths = Vec::with_capacity(4);

    // 1. Project-local fork: .csa/patterns/<name>/
    paths.push(project_root.join(".csa").join("patterns").join(name));

    // 2. Repo-shipped: patterns/<name>/
    paths.push(project_root.join("patterns").join(name));

    // 3. Weave global store: <store_root>/<pkg>/<commit>/patterns/<name>/
    if let Some(store) = store_root {
        if let Some(lockfile_path) = package::find_lockfile(project_root) {
            if let Ok(lockfile) = package::load_lockfile(&lockfile_path) {
                for pkg in &lockfile.package {
                    let commit_key = match pkg.source_kind {
                        SourceKind::Local => "local",
                        SourceKind::Git if pkg.commit.is_empty() => continue,
                        SourceKind::Git => &pkg.commit,
                    };
                    let pkg_dir = package::package_dir(store, &pkg.name, commit_key);
                    paths.push(pkg_dir.join("patterns").join(name));
                }
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

        // Create pattern in global store at <store>/<pkg>/<prefix>/patterns/<name>/
        let pkg_dir = package::package_dir(store.path(), "some-pkg", commit);
        make_pattern_dir(
            &pkg_dir,
            "patterns/csa-review",
            "csa-review",
            "# CSA Review\nGlobal store.",
            None,
        );

        // Write lockfile referencing this package.
        write_lockfile(tmp.path(), "some-pkg", commit);

        let paths = search_paths_with_store("csa-review", tmp.path(), Some(store.path()));
        let found = paths.iter().find(|p| {
            p.join("skills")
                .join("csa-review")
                .join("SKILL.md")
                .is_file()
        });
        assert!(found.is_some(), "pattern not found in global store paths");
        let skill_md =
            fs::read_to_string(found.unwrap().join("skills/csa-review/SKILL.md")).unwrap();
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

        // Also place it in global store.
        let pkg_dir = package::package_dir(store.path(), "pkg", commit);
        make_pattern_dir(
            &pkg_dir,
            "patterns/debate",
            "debate",
            "# Global Store Debate",
            None,
        );
        write_lockfile(tmp.path(), "pkg", commit);

        // Use search_paths_with_store to verify ordering.
        let paths = search_paths_with_store("debate", tmp.path(), Some(store.path()));
        // First matching candidate should be the repo pattern.
        let first_match = paths
            .iter()
            .find(|p| p.join("skills").join("debate").join("SKILL.md").is_file());
        assert!(first_match.is_some());
        let content =
            fs::read_to_string(first_match.unwrap().join("skills/debate/SKILL.md")).unwrap();
        assert!(content.contains("Repo Debate"));
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
