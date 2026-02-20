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
use weave::parser::{AgentConfig, SkillConfig};

// ---------------------------------------------------------------------------
// TOML value merge (top-level shallow, nested tables deep-merge)
// ---------------------------------------------------------------------------

/// Merge `overlay` into `base` in place.  For table values at the top level,
/// sub-tables are recursively merged; all other value types are replaced.
fn merge_toml_tables(base: &mut toml::value::Table, overlay: toml::value::Table) {
    for (key, overlay_val) in overlay {
        match (base.get_mut(&key), overlay_val.clone()) {
            (Some(toml::Value::Table(base_tbl)), toml::Value::Table(over_tbl)) => {
                merge_toml_tables(base_tbl, over_tbl);
            }
            _ => {
                base.insert(key, overlay_val);
            }
        }
    }
}

/// Parse a TOML file at `path` into a `toml::Value::Table`, returning `None`
/// when the file does not exist.
fn read_toml_table(path: &Path) -> Result<Option<toml::value::Table>> {
    if !path.is_file() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let val: toml::Value =
        toml::from_str(&content).with_context(|| format!("failed to parse {}", path.display()))?;
    match val {
        toml::Value::Table(tbl) => Ok(Some(tbl)),
        _ => bail!("{} is not a TOML table", path.display()),
    }
}

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
        // Primary: skills/<name>/SKILL.md (new layout)
        let skill_md_path = dir.join("skills").join(name).join("SKILL.md");
        if skill_md_path.is_file() {
            let skill_md = std::fs::read_to_string(&skill_md_path)
                .with_context(|| format!("failed to read {}", skill_md_path.display()))?;

            let config = load_skill_config(dir, name, project_root)?;

            debug!(pattern_dir = %dir.display(), "Pattern resolved");

            return Ok(ResolvedPattern {
                dir: dir.clone(),
                skill_md,
                config,
            });
        }

        // Fallback: PATTERN.md at pattern root (legacy weave-locked layout)
        let pattern_md_path = dir.join("PATTERN.md");
        if pattern_md_path.is_file() {
            let skill_md = std::fs::read_to_string(&pattern_md_path)
                .with_context(|| format!("failed to read {}", pattern_md_path.display()))?;

            let config = load_skill_config(dir, name, project_root)?;

            debug!(pattern_dir = %dir.display(), "Pattern resolved (PATTERN.md fallback)");

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
            .map(|p| {
                format!(
                    "  - {0}/skills/{name}/SKILL.md\n  - {0}/PATTERN.md",
                    p.display()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    )
}

/// Build the ordered list of directories to search for a pattern.
fn search_paths(name: &str, project_root: &Path) -> Vec<PathBuf> {
    // Warn if legacy .weave/deps/ directory still exists.
    let legacy_deps = project_root.join(".weave").join("deps");
    if legacy_deps.is_dir() {
        tracing::warn!(".weave/deps/ detected \u{2014} run `weave migrate` to use global store");
    }

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
                    if let Ok(pkg_dir) = package::package_dir(store, &pkg.name, commit_key) {
                        paths.push(pkg_dir.join("patterns").join(name));
                    }
                }
            }
        }
    }

    paths
}

/// Load `.skill.toml` with a three-tier config cascade.
///
/// Resolution order (later tiers override earlier):
/// 1. Package-embedded `.skill.toml` inside `pattern_dir`
/// 2. User-level `~/.config/cli-sub-agent/patterns/<name>.toml`
/// 3. Project-level `.csa/patterns/<name>.toml` (config-only file, not dir)
///
/// When the pattern was already resolved from a full directory fork
/// (`.csa/patterns/<name>/`), the project TOML overlay still applies but
/// the base comes from that fork's own `.skill.toml`.
fn load_skill_config(
    pattern_dir: &Path,
    name: &str,
    project_root: &Path,
) -> Result<Option<SkillConfig>> {
    load_skill_config_with_user_dir(
        pattern_dir,
        name,
        project_root,
        user_config_dir().as_deref(),
    )
}

/// Return `~/.config/cli-sub-agent` (or platform equivalent).
fn user_config_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|b| b.config_dir().join("cli-sub-agent"))
}

/// Testable inner function that accepts an explicit user config directory.
fn load_skill_config_with_user_dir(
    pattern_dir: &Path,
    name: &str,
    project_root: &Path,
    user_config: Option<&Path>,
) -> Result<Option<SkillConfig>> {
    // Tier 1: package-embedded .skill.toml
    let mut base = read_toml_table(&pattern_dir.join(".skill.toml"))?;

    // Tier 2: user-level overlay
    if let Some(user_dir) = user_config {
        let user_overlay_path = user_dir.join("patterns").join(format!("{name}.toml"));
        if let Some(overlay) = read_toml_table(&user_overlay_path)? {
            match &mut base {
                Some(tbl) => merge_toml_tables(tbl, overlay),
                None => base = Some(overlay),
            }
        }
    }

    // Tier 3: project-level overlay (.csa/patterns/<name>.toml file)
    let project_overlay_path = project_root
        .join(".csa")
        .join("patterns")
        .join(format!("{name}.toml"));
    if let Some(overlay) = read_toml_table(&project_overlay_path)? {
        match &mut base {
            Some(tbl) => merge_toml_tables(tbl, overlay),
            None => base = Some(overlay),
        }
    }

    match base {
        None => Ok(None),
        Some(tbl) => {
            let val = toml::Value::Table(tbl);
            let config: SkillConfig = val
                .try_into()
                .context("failed to deserialize merged .skill.toml")?;
            Ok(Some(config))
        }
    }
}

#[cfg(test)]
#[path = "pattern_resolver_tests.rs"]
mod tests;
