//! Resolve a pattern by name from standard search paths.
//!
//! Patterns are higher-level constructs that embed skills inside a
//! `patterns/<name>/skills/<name>/` directory layout. This resolver
//! searches for that layout and returns the embedded skill content.
//!
//! Search order (first match wins):
//! 1. Current root: `.csa/patterns/<name>/`
//! 2. Current root: `patterns/<name>/`
//! 3. Superproject root (submodule only): `.csa/patterns/<name>/`
//! 4. Superproject root (submodule only): `patterns/<name>/`
//! 5. `<global_store>/<pkg>/<commit>/patterns/<name>/` from lockfiles under current/superproject
//! 6. Binary-bundled fallback for first-party review/debate/planning patterns

use anyhow::{Context, Result, bail};
use csa_config::paths;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::debug;

use weave::package::{self, SourceKind};
use weave::parser::{AgentConfig, SkillConfig};

struct BundledPattern {
    files: &'static [BundledPatternFile],
}

struct BundledPatternFile {
    path: &'static str,
    contents: &'static [u8],
}

static BUNDLED_WRITE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

static BUNDLED_CSA_REVIEW_FILES: &[BundledPatternFile] = &[
    BundledPatternFile {
        path: ".skill.toml",
        contents: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../patterns/csa-review/.skill.toml"
        )),
    },
    BundledPatternFile {
        path: "PATTERN.md",
        contents: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../patterns/csa-review/PATTERN.md"
        )),
    },
    BundledPatternFile {
        path: "workflow.toml",
        contents: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../patterns/csa-review/workflow.toml"
        )),
    },
    BundledPatternFile {
        path: "skills/csa-review/SKILL.md",
        contents: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../patterns/csa-review/skills/csa-review/SKILL.md"
        )),
    },
    BundledPatternFile {
        path: "skills/csa-review/references/disagreement-escalation.md",
        contents: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../patterns/csa-review/skills/csa-review/references/disagreement-escalation.md"
        )),
    },
    BundledPatternFile {
        path: "skills/csa-review/references/fix-workflow.md",
        contents: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../patterns/csa-review/skills/csa-review/references/fix-workflow.md"
        )),
    },
    BundledPatternFile {
        path: "skills/csa-review/references/output-schema.md",
        contents: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../patterns/csa-review/skills/csa-review/references/output-schema.md"
        )),
    },
    BundledPatternFile {
        path: "skills/csa-review/references/red-team-mode.md",
        contents: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../patterns/csa-review/skills/csa-review/references/red-team-mode.md"
        )),
    },
    BundledPatternFile {
        path: "skills/csa-review/references/review-protocol.md",
        contents: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../patterns/csa-review/skills/csa-review/references/review-protocol.md"
        )),
    },
];

static BUNDLED_DEBATE_FILES: &[BundledPatternFile] = &[
    BundledPatternFile {
        path: ".skill.toml",
        contents: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../patterns/debate/.skill.toml"
        )),
    },
    BundledPatternFile {
        path: "PATTERN.md",
        contents: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../patterns/debate/PATTERN.md"
        )),
    },
    BundledPatternFile {
        path: "workflow.toml",
        contents: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../patterns/debate/workflow.toml"
        )),
    },
    BundledPatternFile {
        path: "skills/debate/SKILL.md",
        contents: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../patterns/debate/skills/debate/SKILL.md"
        )),
    },
];

static BUNDLED_MKTD_FILES: &[BundledPatternFile] = &[
    BundledPatternFile {
        path: ".skill.toml",
        contents: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../patterns/mktd/.skill.toml"
        )),
    },
    BundledPatternFile {
        path: "PATTERN.md",
        contents: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../patterns/mktd/PATTERN.md"
        )),
    },
    BundledPatternFile {
        path: "workflow.toml",
        contents: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../patterns/mktd/workflow.toml"
        )),
    },
    BundledPatternFile {
        path: "skills/mktd/SKILL.md",
        contents: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../patterns/mktd/skills/mktd/SKILL.md"
        )),
    },
];

static BUNDLED_CSA_REVIEW_PATTERN: BundledPattern = BundledPattern {
    files: BUNDLED_CSA_REVIEW_FILES,
};
static BUNDLED_DEBATE_PATTERN: BundledPattern = BundledPattern {
    files: BUNDLED_DEBATE_FILES,
};
static BUNDLED_MKTD_PATTERN: BundledPattern = BundledPattern {
    files: BUNDLED_MKTD_FILES,
};

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

    pub(crate) fn skill_source_dir(&self, name: &str) -> PathBuf {
        let skill_dir = self.dir.join("skills").join(name);
        if skill_dir.join("SKILL.md").is_file() {
            skill_dir
        } else {
            self.dir.clone()
        }
    }
}

/// Resolve a pattern by name, searching standard paths in priority order.
///
/// `project_root` is the working directory / project root for the CSA run.
pub(crate) fn resolve_pattern(name: &str, project_root: &Path) -> Result<ResolvedPattern> {
    resolve_pattern_with_materialization_root(name, project_root, None)
}

fn resolve_pattern_with_materialization_root(
    name: &str,
    project_root: &Path,
    materialization_root: Option<&Path>,
) -> Result<ResolvedPattern> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        bail!("Invalid pattern name: '{name}' (must be a simple name, no path separators)");
    }

    let candidates = search_paths(name, project_root);

    for dir in &candidates {
        if let Some(resolved) = resolve_pattern_from_dir(name, project_root, dir)? {
            return Ok(resolved);
        }
    }

    if let Some(bundled) = bundled_pattern(name) {
        let dir = materialize_bundled_pattern(name, bundled, materialization_root)?;
        if let Some(resolved) = resolve_pattern_from_dir(name, project_root, &dir)? {
            debug!(pattern_dir = %resolved.dir.display(), "Pattern resolved (bundled fallback)");
            return Ok(resolved);
        }
        bail!(
            "Bundled pattern '{name}' materialized to {} but did not contain skills/{name}/SKILL.md or PATTERN.md",
            dir.display()
        );
    }

    bail!(
        "Pattern '{name}' not found. Searched:\n{}",
        format_searched_paths(name, &candidates)
    )
}

fn resolve_pattern_from_dir(
    name: &str,
    project_root: &Path,
    dir: &Path,
) -> Result<Option<ResolvedPattern>> {
    // Primary: skills/<name>/SKILL.md (new layout)
    let skill_md_path = dir.join("skills").join(name).join("SKILL.md");
    if skill_md_path.is_file() {
        let skill_md = std::fs::read_to_string(&skill_md_path)
            .with_context(|| format!("failed to read {}", skill_md_path.display()))?;

        let config = load_skill_config(dir, name, project_root)?;

        debug!(pattern_dir = %dir.display(), "Pattern resolved");

        return Ok(Some(ResolvedPattern {
            dir: dir.to_path_buf(),
            skill_md,
            config,
        }));
    }

    // Fallback: PATTERN.md at pattern root (legacy weave-locked layout)
    let pattern_md_path = dir.join("PATTERN.md");
    if pattern_md_path.is_file() {
        let skill_md = std::fs::read_to_string(&pattern_md_path)
            .with_context(|| format!("failed to read {}", pattern_md_path.display()))?;

        let config = load_skill_config(dir, name, project_root)?;

        debug!(pattern_dir = %dir.display(), "Pattern resolved (PATTERN.md fallback)");

        return Ok(Some(ResolvedPattern {
            dir: dir.to_path_buf(),
            skill_md,
            config,
        }));
    }

    Ok(None)
}

fn format_searched_paths(name: &str, candidates: &[PathBuf]) -> String {
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
}

fn bundled_pattern(name: &str) -> Option<&'static BundledPattern> {
    match name {
        "csa-review" => Some(&BUNDLED_CSA_REVIEW_PATTERN),
        "debate" => Some(&BUNDLED_DEBATE_PATTERN),
        "mktd" => Some(&BUNDLED_MKTD_PATTERN),
        _ => None,
    }
}

fn materialize_bundled_pattern(
    name: &str,
    pattern: &BundledPattern,
    materialization_root: Option<&Path>,
) -> Result<PathBuf> {
    let dest = bundled_pattern_materialization_root(materialization_root).join(name);
    write_bundled_pattern(pattern, &dest)
        .with_context(|| format!("failed to materialize bundled pattern '{name}'"))?;
    Ok(dest)
}

fn bundled_pattern_materialization_root(explicit_root: Option<&Path>) -> PathBuf {
    if let Some(root) = explicit_root {
        return root.to_path_buf();
    }

    for env_name in ["CSA_SESSION_DIR", "CSA_DAEMON_SESSION_DIR"] {
        if let Some(value) = std::env::var_os(env_name)
            && !value.is_empty()
        {
            return PathBuf::from(value).join("bundled-patterns");
        }
    }

    let state_dir = paths::state_dir_write().unwrap_or_else(paths::state_dir_fallback);
    state_dir
        .join("bundled-patterns")
        .join(env!("CARGO_PKG_VERSION"))
}

fn write_bundled_pattern(pattern: &BundledPattern, dest_root: &Path) -> Result<()> {
    for file in pattern.files {
        let dest = dest_root.join(file.path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        atomic_write_bundled_file(&dest, file.contents)?;
    }
    Ok(())
}

/// Write bundled content via a same-directory temp file and atomic rename.
fn atomic_write_bundled_file(dest: &Path, contents: &[u8]) -> Result<()> {
    let parent = dest
        .parent()
        .with_context(|| format!("bundled file has no parent path: {}", dest.display()))?;
    let file_name = dest
        .file_name()
        .with_context(|| format!("bundled file has no file name: {}", dest.display()))?;
    let counter = BUNDLED_WRITE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut tmp_file_name = file_name.to_os_string();
    tmp_file_name.push(format!(".tmp.{}-{counter}", std::process::id()));
    let tmp = parent.join(tmp_file_name);

    if let Err(err) = fs::write(&tmp, contents) {
        let _ = fs::remove_file(&tmp);
        return Err(err).with_context(|| format!("failed to write {}", tmp.display()));
    }

    match fs::rename(&tmp, dest) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::AlreadyExists && dest.exists() => {
            fs::remove_file(&tmp).with_context(|| format!("failed to remove {}", tmp.display()))?;
            Ok(())
        }
        Err(err) => {
            let _ = fs::remove_file(&tmp);
            Err(err).with_context(|| {
                format!("failed to rename {} to {}", tmp.display(), dest.display())
            })
        }
    }
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
    let mut search_roots = Vec::with_capacity(8);
    let repo_roots = discover_repo_roots(project_root);

    // 1. Project-local and ancestor repo roots:
    //    .csa/patterns/<name>/ then patterns/<name>/ for each root.
    for root in &repo_roots {
        search_roots.push(root.join(".csa").join("patterns").join(name));
        search_roots.push(root.join("patterns").join(name));
    }

    // 3. Weave global store: <store_root>/<pkg>/<commit>/patterns/<name>/
    if let Some(store) = store_root {
        for root in &repo_roots {
            if let Some(lockfile_path) = package::find_lockfile(root)
                && let Ok(lockfile) = package::load_lockfile(&lockfile_path)
            {
                for pkg in &lockfile.package {
                    let commit_key = match pkg.source_kind {
                        SourceKind::Local => "local",
                        SourceKind::Git if pkg.commit.is_empty() => continue,
                        SourceKind::Git => &pkg.commit,
                    };
                    if let Ok(pkg_dir) = package::package_dir(store, &pkg.name, commit_key) {
                        search_roots.push(pkg_dir.join("patterns").join(name));
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
    paths::config_dir()
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

#[cfg(test)]
#[path = "pattern_resolver_tests_2305.rs"]
mod tests_2305;
