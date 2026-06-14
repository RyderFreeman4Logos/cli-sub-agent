//! Resolve a skill by name from standard search paths.
//!
//! Search scope:
//! - Current project root
//! - Optional superproject root when current root is a git submodule
//!
//! Search order:
//! 1. `./.csa/skills/<name>/`                             (project-local, CSA-specific)
//! 2. `./.claude/skills/<name>/`                          (project-local, Claude Code compat)
//! 3. `./.codex/skills/<name>/`                           (project-local, Codex compat)
//! 4. `./.agents/skills/<name>/`                          (project-local, shared agent compat)
//! 5. `./skills/csa/<name>/` and `./.claude/skills/csa/<name>/`
//! 6. Matching active skill directories and bundled namespaces in an optional superproject root
//! 7. `~/.claude/skills/<name>/`, `~/.codex/skills/<name>/`, `~/.agents/skills/<name>/`
//! 8. `~/.config/cli-sub-agent/skills/<name>/`            (global user config)
//! 9. `~/.local/state/cli-sub-agent/skills/<name>/`       (CSA-managed inactive skills)
//! 10. `<global_store>/<name>/<commit>/`                  (weave global store via lockfiles)

use anyhow::{Context, Result, bail};
use csa_config::paths;
use std::path::{Path, PathBuf};

use weave::package::{self, SourceKind};
use weave::parser::{AgentConfig, SkillConfig, parse_skill_config};

use crate::skill_repo::sanitize_skill_md;

const CSA_SKILL_DIR: &str = ".csa/skills";

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

/// A runnable skill discovered for `csa skill list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActiveSkillSource {
    /// Skill name used by `csa run --skill <name>`.
    pub name: String,
    /// Directory where the skill lives.
    pub dir: PathBuf,
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
    resolve_skill_from_candidates(name, &candidates)
}

fn resolve_skill_from_candidates(name: &str, candidates: &[PathBuf]) -> Result<ResolvedSkill> {
    for dir in candidates {
        let skill_md_path = dir.join("SKILL.md");
        if skill_md_path.is_file() {
            let raw_skill_md = std::fs::read_to_string(&skill_md_path)
                .with_context(|| format!("failed to read {}", skill_md_path.display()))?;

            // Strip prompt-injection tags before the content reaches any LLM.
            let skill_md = sanitize_skill_md(&raw_skill_md);

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

/// List active runnable skills using the same runnable non-store parent
/// directories searched by the run resolver.
pub(crate) fn list_active_skill_sources(project_root: &Path) -> Result<Vec<ActiveSkillSource>> {
    list_active_skill_sources_with_home_and_config(
        project_root,
        user_home_dir().as_deref(),
        paths::config_dir().as_deref(),
    )
}

/// Build the ordered list of directories to search for a skill.
fn search_paths(name: &str, project_root: &Path) -> Vec<PathBuf> {
    search_paths_with_store(
        name,
        project_root,
        package::global_store_root().ok().as_deref(),
        user_home_dir().as_deref(),
    )
}

/// Build search paths using an explicit store root (testable).
fn search_paths_with_store(
    name: &str,
    project_root: &Path,
    store_root: Option<&Path>,
    home_dir: Option<&Path>,
) -> Vec<PathBuf> {
    search_paths_with_explicit_dirs(
        name,
        project_root,
        store_root,
        home_dir,
        paths::config_dir().as_deref(),
        paths::state_dir_write().as_deref(),
    )
}

fn search_paths_with_explicit_dirs(
    name: &str,
    project_root: &Path,
    store_root: Option<&Path>,
    home_dir: Option<&Path>,
    config_dir: Option<&Path>,
    state_dir: Option<&Path>,
) -> Vec<PathBuf> {
    let repo_roots = discover_repo_roots(project_root);
    let mut parent_dirs =
        runnable_non_store_skill_parent_dirs_with_config(&repo_roots, home_dir, config_dir);
    if let Some(state_dir) = state_dir {
        parent_dirs.push(state_dir.join("skills"));
    }
    let mut search_roots = parent_dirs
        .into_iter()
        .map(|parent| parent.join(name))
        .collect::<Vec<_>>();

    // Weave global store: match locked packages by name.
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

fn runnable_non_store_skill_parent_dirs_with_config(
    repo_roots: &[PathBuf],
    home_dir: Option<&Path>,
    config_dir: Option<&Path>,
) -> Vec<PathBuf> {
    let mut dirs = Vec::with_capacity(repo_roots.len() * 6 + 5);

    for root in repo_roots {
        dirs.extend(repo_active_skill_parent_dirs(root));
        dirs.push(root.join("skills").join("csa"));
        dirs.push(root.join(".claude").join("skills").join("csa"));
    }
    if let Some(home) = home_dir {
        dirs.extend(home_active_skill_parent_dirs(home));
    }
    if let Some(config_dir) = config_dir {
        dirs.push(config_dir.join("skills"));
    }

    dirs
}

fn repo_active_skill_parent_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::with_capacity(weave::check::DEFAULT_LINK_DIRS.len() + 1);
    dirs.push(root.join(CSA_SKILL_DIR));
    dirs.extend(
        weave::check::DEFAULT_LINK_DIRS
            .iter()
            .map(|rel| root.join(rel)),
    );
    dirs
}

fn home_active_skill_parent_dirs(home: &Path) -> Vec<PathBuf> {
    weave::check::DEFAULT_LINK_DIRS
        .iter()
        .map(|rel| home.join(rel))
        .collect()
}

fn list_active_skill_sources_with_home_and_config(
    project_root: &Path,
    home_dir: Option<&Path>,
    config_dir: Option<&Path>,
) -> Result<Vec<ActiveSkillSource>> {
    let mut active_skills = Vec::new();
    let repo_roots = discover_repo_roots(project_root);
    for dir in runnable_non_store_skill_parent_dirs_with_config(&repo_roots, home_dir, config_dir) {
        if !dir.exists() {
            continue;
        }
        for entry in std::fs::read_dir(&dir)?.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) if !n.starts_with('.') => n.to_string(),
                _ => continue,
            };
            if path.join("SKILL.md").exists() {
                active_skills.push(ActiveSkillSource { name, dir: path });
            }
        }
    }

    active_skills.sort_by(|a, b| a.name.cmp(&b.name));
    active_skills.dedup_by(|a, b| a.name == b.name);
    Ok(active_skills)
}

fn user_home_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf())
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
#[path = "skill_resolver_tests.rs"]
mod skill_resolver_tests;
