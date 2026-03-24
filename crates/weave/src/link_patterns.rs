//! Pattern directory linking for weave packages.
//!
//! Discovers pattern directories (containing `workflow.toml`) in installed packages
//! and creates symlinks in the consumer project's `patterns/` directory.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::warn;

use super::{
    DiscoveredPattern, DiscoveredSkill, LinkOutcome, LinkReport, create_skill_link,
    is_weave_managed_path,
};
use crate::package::{SourceKind, find_lockfile, global_store_root, load_lockfile, package_dir};
use crate::path_utils::resolve_symlink_target;

/// Discover all pattern directories (containing workflow.toml) across installed packages.
///
/// Includes the package name for conflict detection.
pub fn discover_patterns(project_root: &Path) -> Result<Vec<DiscoveredPattern>> {
    let store_root = global_store_root()?;
    let lockfile = match find_lockfile(project_root) {
        Some(path) => load_lockfile(&path)?,
        None => return Ok(Vec::new()),
    };

    let mut patterns = Vec::new();
    for pkg in &lockfile.package {
        let commit_key = if pkg.source_kind == SourceKind::Local {
            "local"
        } else if pkg.commit.is_empty() {
            continue;
        } else {
            pkg.commit.as_str()
        };
        let pkg_dir = match package_dir(&store_root, &pkg.name, commit_key) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let patterns_dir = pkg_dir.join("patterns");
        if !patterns_dir.is_dir() {
            continue;
        }
        let entries = match std::fs::read_dir(&patterns_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let dir = entry.path();
            if dir.is_dir()
                && dir.join("workflow.toml").is_file()
                && let Some(name) = dir.file_name().map(|n| n.to_string_lossy().to_string())
            {
                patterns.push(DiscoveredPattern {
                    name,
                    package_name: pkg.name.clone(),
                    source_dir: dir,
                });
            }
        }
    }
    Ok(patterns)
}

/// Create symlinks for discovered patterns in the project's `patterns/` directory.
///
/// Each pattern gets a symlink: `patterns/<name>` -> `<global_store>/patterns/<name>`.
/// Skips patterns whose target already exists locally as a real directory (not symlink).
/// `force` allows overwriting non-weave-managed symlinks.
pub fn link_patterns(project_root: &Path, force: bool) -> Result<LinkReport> {
    let patterns = discover_patterns(project_root)?;
    let store_root = global_store_root()?;
    let target_dir = project_root.join("patterns");
    let mut report = LinkReport::default();

    if patterns.is_empty() {
        return Ok(report);
    }

    // Conflict detection: warn if two packages provide the same pattern name.
    let mut seen: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for pat in &patterns {
        if let Some(&existing_pkg) = seen.get(pat.name.as_str())
            && existing_pkg != pat.package_name
        {
            warn!(
                "pattern '{}' provided by both '{}' and '{}'; using first match",
                pat.name, existing_pkg, pat.package_name
            );
            continue;
        }
        seen.insert(&pat.name, &pat.package_name);
    }

    for pat in &patterns {
        let link_path = target_dir.join(&pat.name);

        // Skip if a real (non-symlink) directory already exists — local override.
        if link_path.exists()
            && !link_path
                .symlink_metadata()
                .map(|m| m.is_symlink())
                .unwrap_or(false)
        {
            report.outcomes.push(LinkOutcome::Skipped {
                name: pat.name.clone(),
            });
            continue;
        }

        let skill_proxy = DiscoveredSkill {
            name: pat.name.clone(),
            package_name: pat.package_name.clone(),
            source_dir: pat.source_dir.clone(),
        };

        if !target_dir.exists() {
            std::fs::create_dir_all(&target_dir)
                .with_context(|| format!("cannot create {}", target_dir.display()))?;
        }

        match create_skill_link(
            &link_path,
            &pat.source_dir,
            &target_dir,
            &store_root,
            &skill_proxy,
            force,
        ) {
            Ok(o) => report.outcomes.push(o),
            Err(e) => report.errors.push(e),
        }
    }

    Ok(report)
}

/// Remove stale pattern symlinks that point into the weave store but whose
/// pattern is no longer provided by any installed package.
pub fn remove_stale_pattern_links(project_root: &Path) -> Result<Vec<PathBuf>> {
    let store_root = global_store_root()?;
    let current_patterns = discover_patterns(project_root)?;
    let current_names: std::collections::HashSet<&str> =
        current_patterns.iter().map(|p| p.name.as_str()).collect();

    let patterns_dir = project_root.join("patterns");
    if !patterns_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut stale = Vec::new();
    let entries = match std::fs::read_dir(&patterns_dir) {
        Ok(e) => e,
        Err(_) => return Ok(Vec::new()),
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.is_symlink() {
            continue;
        }
        let name = match path.file_name().map(|n| n.to_string_lossy().to_string()) {
            Some(n) => n,
            None => continue,
        };
        // Only remove if it points into the weave store AND is not in current patterns.
        let target = match std::fs::read_link(&path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let resolved = resolve_symlink_target(&patterns_dir, &target);
        if is_weave_managed_path(&resolved, &store_root)
            && !current_names.contains(name.as_str())
            && std::fs::remove_file(&path).is_ok()
        {
            stale.push(path);
        }
    }

    Ok(stale)
}
