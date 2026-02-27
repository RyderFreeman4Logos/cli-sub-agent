//! Weave git-native package management.
//!
//! Skills are distributed as git repositories. Weave clones them into a
//! content-addressable cache (`~/.cache/weave/git/<url-hash>/`) and checks
//! out the requested revision into the global package store at
//! `~/.local/share/weave/packages/<name>/<commit-prefix>/`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[path = "package_git.rs"]
mod package_git;
pub use package_git::{
    default_cache_root, find_lockfile, global_store_root, is_checkout_valid,
    load_project_lockfile, lockfile_path,
};
pub(crate) use package_git::detect_skill_md_case_mismatch;
use package_git::{
    checkout_to, copy_dir_recursive, ensure_cached, legacy_lockfile_path, read_version,
    resolve_commit,
};

/// Root structure of the lockfile (`weave.lock`).
///
/// The lock file may also contain CSA version/migration tracking sections
/// (`[versions]`, `[migrations]`). These are preserved as opaque TOML values
/// so that package operations do not discard them.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Lockfile {
    #[serde(default)]
    pub package: Vec<LockedPackage>,
    /// CSA version tracking — preserved across load/save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub versions: Option<toml::Value>,
    /// CSA migration tracking — preserved across load/save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migrations: Option<toml::Value>,
}

impl Lockfile {
    /// Create a lockfile with only package entries (no version tracking).
    pub fn with_packages(package: Vec<LockedPackage>) -> Self {
        Self {
            package,
            ..Default::default()
        }
    }
}

/// How a dependency was installed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    /// Installed from a git repository (default for backward compatibility).
    #[default]
    Git,
    /// Installed from a local directory path.
    Local,
}

/// A single locked dependency.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LockedPackage {
    pub name: String,
    pub repo: String,
    pub commit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// How this dependency was installed. Defaults to `Git` for backward
    /// compatibility with lockfiles that predate this field.
    #[serde(default)]
    pub source_kind: SourceKind,
    /// User-requested version specifier (e.g. `v1.2.0`, `main`, `abc123`).
    /// When set, the dependency is considered "pinned" and `update` will skip
    /// it unless `--force` is passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_version: Option<String>,
    /// The git ref that was resolved during install (branch, tag, or commit
    /// hash before full resolution). Absent means HEAD was used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_ref: Option<String>,
}

/// Parsed install source — a git URL with an optional ref and skill name.
#[derive(Debug, Clone, PartialEq)]
pub struct InstallSource {
    /// Canonical git URL (https).
    pub url: String,
    /// Git ref to checkout (branch/tag/commit). None means HEAD.
    pub git_ref: Option<String>,
    /// Skill name (last path segment without `.git`).
    pub name: String,
}

/// Parse a source string into an `InstallSource`.
///
/// Accepted formats:
/// - `user/repo` → `https://github.com/user/repo.git`
/// - `github.com/user/repo` → `https://github.com/user/repo.git`
/// - `https://github.com/user/repo` → as-is with `.git` suffix
/// - `https://github.com/user/repo@v1.0` → with ref
/// - `https://github.com/user/repo#branch` → with ref
pub fn parse_source(source: &str) -> Result<InstallSource> {
    let (url_part, git_ref) = if let Some((url, r)) = source.rsplit_once('@') {
        (url.to_string(), Some(r.to_string()))
    } else if let Some((url, r)) = source.rsplit_once('#') {
        (url.to_string(), Some(r.to_string()))
    } else {
        (source.to_string(), None)
    };

    let url = normalize_url(&url_part)?;
    let name = extract_name(&url)?;

    Ok(InstallSource { url, git_ref, name })
}

/// Normalize various URL formats to canonical https git URL.
fn normalize_url(input: &str) -> Result<String> {
    // Already a full URL
    if input.starts_with("https://") || input.starts_with("http://") {
        let url = if input.ends_with(".git") {
            input.to_string()
        } else {
            format!("{input}.git")
        };
        return Ok(url);
    }

    // domain/user/repo format (e.g., github.com/user/repo)
    if input.contains('.') && input.contains('/') {
        let url = if input.ends_with(".git") {
            format!("https://{input}")
        } else {
            format!("https://{input}.git")
        };
        return Ok(url);
    }

    // user/repo shorthand → GitHub
    let parts: Vec<&str> = input.split('/').collect();
    if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
        return Ok(format!("https://github.com/{}/{}.git", parts[0], parts[1]));
    }

    bail!("cannot parse source: '{input}' (expected user/repo, domain/user/repo, or full URL)")
}

/// Extract the skill name from a git URL (last path segment minus `.git`).
fn extract_name(url: &str) -> Result<String> {
    let path = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);

    let last_segment = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .context("empty URL path")?;

    let name = last_segment.strip_suffix(".git").unwrap_or(last_segment);

    if name.is_empty() {
        bail!("could not extract skill name from URL: {url}");
    }

    Ok(name.to_string())
}

// ---------------------------------------------------------------------------
// Package store validation
// ---------------------------------------------------------------------------

/// Validate that a package name contains only `[a-zA-Z0-9_-]`.
pub fn validate_package_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("package name must not be empty");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        bail!("invalid package name '{name}': only [a-zA-Z0-9_-] allowed");
    }
    Ok(())
}

/// Validate that a commit string is hex-only or the literal `"local"`.
fn validate_commit_prefix(commit: &str) -> Result<()> {
    if commit == "local" {
        return Ok(());
    }
    if commit.is_empty() || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("invalid commit '{commit}': only hex characters [0-9a-f] allowed");
    }
    Ok(())
}

/// Compute the checkout directory: `<store_root>/<name>/<commit_prefix>/`.
///
/// Returns an error if `name` or `commit` contain path-traversal characters.
pub fn package_dir(store_root: &Path, name: &str, commit: &str) -> Result<PathBuf> {
    validate_package_name(name)?;
    let prefix_len = commit.len().min(8);
    validate_commit_prefix(&commit[..prefix_len])?;
    Ok(store_root.join(name).join(&commit[..prefix_len]))
}

// ---------------------------------------------------------------------------
// Install
// ---------------------------------------------------------------------------

/// Install a skill from a git source into the global package store.
///
/// Checkout goes to `<store_root>/<name>/<commit-prefix>/`.
/// Skips checkout when the destination already contains a valid checkout
/// (content-addressed idempotency).
///
/// Returns the locked package entry.
pub fn install(
    source: &str,
    project_root: &Path,
    cache_root: &Path,
    store_root: &Path,
) -> Result<LockedPackage> {
    let src = parse_source(source)?;
    let cas = ensure_cached(cache_root, &src.url)?;
    let commit = resolve_commit(&cas, src.git_ref.as_deref())?;

    let dest = package_dir(store_root, &src.name, &commit)?;
    if !is_checkout_valid(&dest) {
        checkout_to(&cas, &commit, &dest)?;
    }

    let version = read_version(&dest);

    let pkg = LockedPackage {
        name: src.name,
        repo: src.url,
        commit,
        version,
        source_kind: SourceKind::Git,
        requested_version: src.git_ref.clone(),
        resolved_ref: src.git_ref,
    };

    // Update the lockfile with this package.
    let lock_path = lockfile_path(project_root);
    let mut lockfile = load_project_lockfile(project_root).unwrap_or_default();
    upsert_package(&mut lockfile, &pkg);
    save_lockfile(&lock_path, &lockfile)?;

    Ok(pkg)
}

/// Install a skill from a local directory path into the global package store.
///
/// The source directory is recursively copied (excluding `.git/`).
/// Returns the locked package entry with `source_kind = Local`.
pub fn install_from_local(
    source_path: &Path,
    project_root: &Path,
    store_root: &Path,
) -> Result<LockedPackage> {
    let canonical = source_path
        .canonicalize()
        .with_context(|| format!("cannot resolve path: {}", source_path.display()))?;

    if !canonical.is_dir() {
        bail!("not a directory: {}", canonical.display());
    }

    // Extract name from the directory basename.
    let name = canonical
        .file_name()
        .context("cannot extract directory name")?
        .to_string_lossy()
        .to_string();

    // Validate name — no path separators or traversal.
    if name.contains('/') || name.contains('\\') || name == ".." || name == "." || name.is_empty() {
        bail!("invalid skill name: '{name}'");
    }

    // Require SKILL.md to be a regular file (not a symlink — copy skips symlinks).
    let skill_md = canonical.join("SKILL.md");
    match std::fs::symlink_metadata(&skill_md) {
        Ok(m) if m.file_type().is_file() => {} // regular file — ok
        Ok(m) if m.file_type().is_symlink() => {
            bail!(
                "SKILL.md in {} is a symlink — symlinks are not copied during install",
                canonical.display()
            );
        }
        _ => {
            // Check for a case-mismatched variant before reporting "not found".
            if let Some(found) = detect_skill_md_case_mismatch(&canonical) {
                bail!(
                    "expected 'SKILL.md' but found '{found}' in {} (wrong case). \
                     Rename to 'SKILL.md' to fix.",
                    canonical.display()
                );
            }
            bail!(
                "SKILL.md not found in {} — not a valid skill directory",
                canonical.display()
            );
        }
    }

    // Resolve the store root through any symlinks so overlap detection
    // works correctly even when the store path contains symlinks.
    let resolved_store = if store_root.exists() {
        store_root.canonicalize()?
    } else {
        store_root.to_path_buf()
    };
    // Local sources use "local" as the commit prefix in the global store.
    let dest = package_dir(&resolved_store, &name, "local")?;

    // Guard against source/destination overlap.
    {
        let dest_approx = if dest.parent().is_some_and(|p| p.exists()) {
            dest.parent()
                .unwrap()
                .canonicalize()?
                .join(dest.file_name().unwrap_or_default())
        } else {
            dest.clone()
        };
        if canonical == dest_approx
            || canonical.starts_with(&dest_approx)
            || dest_approx.starts_with(&canonical)
        {
            bail!(
                "source and destination overlap: {} vs {}",
                canonical.display(),
                dest_approx.display()
            );
        }
    }

    // Copy to a staging directory first, then swap — ensures the original
    // is preserved if the copy fails (atomic-ish replace).
    let parent = dest.parent().context("invalid global store path")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("failed to create {}", parent.display()))?;
    let staging = parent.join(format!(".local.staging.{}", std::process::id()));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    copy_dir_recursive(&canonical, &staging).inspect_err(|_| {
        // Clean up partial staging on failure.
        let _ = std::fs::remove_dir_all(&staging);
    })?;

    // Swap: remove old, rename staging → dest.
    if dest.exists() {
        std::fs::remove_dir_all(&dest)
            .with_context(|| format!("failed to remove existing {}", dest.display()))?;
    }
    std::fs::rename(&staging, &dest)
        .with_context(|| format!("failed to rename staging to {}", dest.display()))?;

    let version = read_version(&dest);

    let pkg = LockedPackage {
        name,
        repo: String::new(),
        commit: String::new(),
        version,
        source_kind: SourceKind::Local,
        requested_version: None,
        resolved_ref: None,
    };

    // Update the lockfile.
    let lock_path = lockfile_path(project_root);
    let mut lockfile = load_project_lockfile(project_root).unwrap_or_default();
    upsert_package(&mut lockfile, &pkg);
    save_lockfile(&lock_path, &lockfile)?;

    Ok(pkg)
}

// ---------------------------------------------------------------------------
// Lock & Update
// ---------------------------------------------------------------------------

/// Regenerate the lockfile from the current lockfile state and global store.
///
/// For each existing lockfile entry, verify the checkout exists in the
/// global store and update the version if changed. Entries whose checkouts
/// are missing from the store are retained (audit will flag them).
pub fn lock(project_root: &Path, store_root: &Path) -> Result<Lockfile> {
    let lock_path = lockfile_path(project_root);

    let existing = load_project_lockfile(project_root).unwrap_or_default();

    let mut packages = Vec::new();

    for pkg in existing.package {
        let mut updated = pkg.clone();
        // Determine the checkout directory in the global store.
        let commit_key = if pkg.source_kind == SourceKind::Local {
            "local"
        } else if pkg.commit.is_empty() {
            ""
        } else {
            &pkg.commit
        };
        if !commit_key.is_empty() {
            let checkout = package_dir(store_root, &pkg.name, commit_key)?;
            if checkout.is_dir() {
                updated.version = read_version(&checkout);
            }
        }
        packages.push(updated);
    }

    let lockfile = Lockfile {
        package: packages,
        versions: existing.versions,
        migrations: existing.migrations,
    };
    save_lockfile(&lock_path, &lockfile)?;

    Ok(lockfile)
}

/// Update one or all locked dependencies to their latest commit.
///
/// When `force` is false, dependencies with a `requested_version` (pinned)
/// are skipped. When `force` is true, pinned dependencies are re-fetched
/// and re-resolved from their pinned ref (not HEAD).
pub fn update(
    name: Option<&str>,
    project_root: &Path,
    cache_root: &Path,
    store_root: &Path,
    force: bool,
) -> Result<Vec<LockedPackage>> {
    let lock_path = lockfile_path(project_root);
    let mut lockfile = load_project_lockfile(project_root)
        .context("no lockfile found — run `weave lock` first")?;

    let targets: Vec<usize> = if let Some(n) = name {
        let idx = lockfile
            .package
            .iter()
            .position(|p| p.name == n)
            .with_context(|| format!("package '{n}' not found in lockfile"))?;
        vec![idx]
    } else {
        (0..lockfile.package.len()).collect()
    };

    let mut updated = Vec::new();

    for idx in targets {
        let pkg = &lockfile.package[idx];
        if pkg.source_kind == SourceKind::Local {
            eprintln!(
                "skipping {} (local source — reinstall with --path to update)",
                pkg.name
            );
            continue;
        }
        if pkg.repo.is_empty() {
            continue; // Skip entries without a known repo.
        }

        // Skip pinned dependencies unless --force is used.
        if pkg.requested_version.is_some() && !force {
            eprintln!(
                "skipping {} (pinned to {} — use --force to override)",
                pkg.name,
                pkg.requested_version.as_deref().unwrap_or("?")
            );
            continue;
        }

        let cas = ensure_cached(cache_root, &pkg.repo)?;

        // When --force is used on pinned deps, resolve from the configured ref
        // (branch/tag name) instead of the previously resolved commit hash,
        // so we can advance past immutable pinned refs.
        let resolve_ref = if force {
            pkg.requested_version.as_deref()
        } else {
            pkg.resolved_ref.as_deref()
        };
        let new_commit = resolve_commit(&cas, resolve_ref)?;

        if new_commit != pkg.commit {
            let dest = package_dir(store_root, &pkg.name, &new_commit)?;
            if !is_checkout_valid(&dest) {
                checkout_to(&cas, &new_commit, &dest)?;
            }

            let version = read_version(&dest);
            lockfile.package[idx].commit = new_commit;
            lockfile.package[idx].version = version;
        }

        updated.push(lockfile.package[idx].clone());
    }

    save_lockfile(&lock_path, &lockfile)?;
    Ok(updated)
}

// ---------------------------------------------------------------------------
// Upgrade
// ---------------------------------------------------------------------------

/// Per-package outcome of an upgrade operation.
#[derive(Debug, Clone, PartialEq)]
pub enum UpgradeStatus {
    /// Package was upgraded from `old_commit` to `new_commit`.
    Upgraded {
        old_commit: String,
        old_version: Option<String>,
    },
    /// Package was already at the latest commit.
    AlreadyLatest,
    /// Package was skipped (local source, empty repo, or pinned).
    Skipped { reason: String },
}

/// Result of upgrading a single package.
#[derive(Debug, Clone, PartialEq)]
pub struct UpgradeEntry {
    pub name: String,
    pub status: UpgradeStatus,
    /// Current package state after upgrade attempt.
    pub package: LockedPackage,
}

/// Upgrade all installed packages to their latest available versions.
///
/// Unlike `update`, this function returns structured results that distinguish
/// between packages that were upgraded, already at latest, or skipped.
/// Pinned packages are skipped unless `force` is true.
pub fn upgrade(
    project_root: &Path,
    cache_root: &Path,
    store_root: &Path,
    force: bool,
) -> Result<Vec<UpgradeEntry>> {
    let lock_path = lockfile_path(project_root);
    let mut lockfile = load_project_lockfile(project_root)
        .context("no lockfile found — run `weave install` first")?;

    let mut results = Vec::new();

    for idx in 0..lockfile.package.len() {
        let pkg = &lockfile.package[idx];

        if pkg.source_kind == SourceKind::Local {
            results.push(UpgradeEntry {
                name: pkg.name.clone(),
                status: UpgradeStatus::Skipped {
                    reason: "local source — reinstall with --path to update".to_string(),
                },
                package: pkg.clone(),
            });
            continue;
        }

        if pkg.repo.is_empty() {
            results.push(UpgradeEntry {
                name: pkg.name.clone(),
                status: UpgradeStatus::Skipped {
                    reason: "no repository URL".to_string(),
                },
                package: pkg.clone(),
            });
            continue;
        }

        if pkg.requested_version.is_some() && !force {
            results.push(UpgradeEntry {
                name: pkg.name.clone(),
                status: UpgradeStatus::Skipped {
                    reason: format!(
                        "pinned to {} — use --force to override",
                        pkg.requested_version.as_deref().unwrap_or("?")
                    ),
                },
                package: pkg.clone(),
            });
            continue;
        }

        let cas = ensure_cached(cache_root, &pkg.repo)?;

        // When --force is used on pinned deps, resolve from the configured ref
        // (branch/tag name) instead of the previously resolved commit hash,
        // so we can advance past immutable pinned refs.
        let resolve_ref = if force {
            pkg.requested_version.as_deref()
        } else {
            pkg.resolved_ref.as_deref()
        };
        let new_commit = resolve_commit(&cas, resolve_ref)?;

        if new_commit != pkg.commit {
            let old_commit = pkg.commit.clone();
            let old_version = pkg.version.clone();

            let dest = package_dir(store_root, &pkg.name, &new_commit)?;
            if !is_checkout_valid(&dest) {
                checkout_to(&cas, &new_commit, &dest)?;
            }

            let version = read_version(&dest);
            lockfile.package[idx].commit = new_commit;
            lockfile.package[idx].version = version;

            results.push(UpgradeEntry {
                name: lockfile.package[idx].name.clone(),
                status: UpgradeStatus::Upgraded {
                    old_commit,
                    old_version,
                },
                package: lockfile.package[idx].clone(),
            });
        } else {
            results.push(UpgradeEntry {
                name: pkg.name.clone(),
                status: UpgradeStatus::AlreadyLatest,
                package: pkg.clone(),
            });
        }
    }

    save_lockfile(&lock_path, &lockfile)?;
    Ok(results)
}

#[path = "package_migrate.rs"]
mod package_migrate;
pub use package_migrate::{LegacyDir, MigrateResult, migrate};

#[path = "package_audit.rs"]
mod package_audit;
pub use package_audit::{AuditIssue, AuditResult, audit};

#[path = "package_gc.rs"]
mod package_gc;
pub use package_gc::{GcResult, gc};

/// Load a lockfile from disk.
pub fn load_lockfile(path: &Path) -> Result<Lockfile> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_str(&content).with_context(|| format!("failed to parse {}", path.display()))
}

/// Save a lockfile to disk.
pub fn save_lockfile(path: &Path, lockfile: &Lockfile) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let content = toml::to_string_pretty(lockfile).context("failed to serialize lockfile")?;
    std::fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))
}

/// Insert or update a package in the lockfile.
fn upsert_package(lockfile: &mut Lockfile, pkg: &LockedPackage) {
    if let Some(existing) = lockfile.package.iter_mut().find(|p| p.name == pkg.name) {
        *existing = pkg.clone();
    } else {
        lockfile.package.push(pkg.clone());
    }
}

#[cfg(test)]
#[path = "package_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "package_install_tests.rs"]
mod install_tests;

#[cfg(test)]
#[path = "package_security_tests.rs"]
mod security_tests;

#[cfg(test)]
#[path = "package_upgrade_tests.rs"]
mod upgrade_tests;

#[cfg(test)]
#[path = "package_tests_audit.rs"]
mod audit_tests;
