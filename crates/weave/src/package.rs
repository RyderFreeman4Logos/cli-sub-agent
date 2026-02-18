//! Weave git-native package management.
//!
//! Skills are distributed as git repositories. Weave clones them into a
//! content-addressable cache (`~/.cache/weave/git/<url-hash>/`) and checks
//! out the requested revision into `.weave/deps/<name>/`.
//!
//! Commands:
//! - `install <source>` — clone/fetch + checkout into deps
//! - `lock` — snapshot current deps into `weave.lock`
//! - `update [name]` — fetch latest and re-lock
//! - `audit` — verify lockfile consistency

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Lockfile types
// ---------------------------------------------------------------------------

/// Root structure of the lockfile (`weave.lock`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Lockfile {
    #[serde(default)]
    pub package: Vec<LockedPackage>,
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

// ---------------------------------------------------------------------------
// Source parsing
// ---------------------------------------------------------------------------

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
// CAS cache
// ---------------------------------------------------------------------------

/// Compute the CAS cache directory for a git URL.
///
/// Uses a simple deterministic path: `~/.cache/weave/git/<safe-name>/`
/// where safe-name is the URL with special chars replaced.
fn cas_dir_for(cache_root: &Path, url: &str) -> PathBuf {
    // Create a filesystem-safe key from the URL.
    let safe: String = url
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    cache_root.join("git").join(safe)
}

/// Return the default cache root: `~/.cache/weave/`.
pub fn default_cache_root() -> Result<PathBuf> {
    let base = directories::BaseDirs::new().context("cannot determine home directory")?;
    Ok(base.cache_dir().join("weave"))
}

// ---------------------------------------------------------------------------
// Lockfile path helpers
// ---------------------------------------------------------------------------

/// Canonical lockfile path: `<project_root>/weave.lock`.
pub fn lockfile_path(project_root: &Path) -> PathBuf {
    project_root.join("weave.lock")
}

/// Legacy lockfile path: `<project_root>/.weave/lock.toml`.
fn legacy_lockfile_path(project_root: &Path) -> PathBuf {
    project_root.join(".weave").join("lock.toml")
}

/// Find the lockfile, preferring the new path over the legacy one.
///
/// Returns `Some(path)` if a lockfile exists at either location, `None` otherwise.
pub fn find_lockfile(project_root: &Path) -> Option<PathBuf> {
    let new_path = lockfile_path(project_root);
    if new_path.is_file() {
        return Some(new_path);
    }
    let old_path = legacy_lockfile_path(project_root);
    if old_path.is_file() {
        return Some(old_path);
    }
    None
}

/// Load the project lockfile, searching both new and legacy paths.
pub fn load_project_lockfile(project_root: &Path) -> Result<Lockfile> {
    match find_lockfile(project_root) {
        Some(path) => load_lockfile(&path),
        None => bail!("no lockfile found at {} or {}", lockfile_path(project_root).display(), legacy_lockfile_path(project_root).display()),
    }
}

// ---------------------------------------------------------------------------
// Global package store
// ---------------------------------------------------------------------------

/// Return the global store root: `~/.local/share/weave/packages/`.
pub fn global_store_root() -> Result<PathBuf> {
    let base = directories::BaseDirs::new().context("cannot determine home directory")?;
    Ok(base.data_local_dir().join("weave").join("packages"))
}

/// Compute the checkout directory for a package in the global store.
///
/// Layout: `<store_root>/<name>/<commit_prefix>/` where commit_prefix is
/// the first 8 characters of the commit hash.
pub fn package_dir(store_root: &Path, name: &str, commit: &str) -> PathBuf {
    let prefix_len = commit.len().min(8);
    store_root.join(name).join(&commit[..prefix_len])
}

/// Check whether a checkout directory is valid (exists and contains at
/// least one file).
pub fn is_checkout_valid(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    match std::fs::read_dir(dir) {
        Ok(mut entries) => entries.any(|e| e.is_ok()),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Git operations
// ---------------------------------------------------------------------------

/// Clone or fetch a bare repository in the CAS cache. Returns the CAS path.
fn ensure_cached(cache_root: &Path, url: &str) -> Result<PathBuf> {
    let cas = cas_dir_for(cache_root, url);

    if cas.join("HEAD").is_file() {
        // Already cloned — fetch updates.
        let status = Command::new("git")
            .args(["fetch", "--quiet", "--all"])
            .current_dir(&cas)
            .status()
            .context("failed to run git fetch")?;
        if !status.success() {
            bail!("git fetch failed in {}", cas.display());
        }
    } else {
        // Fresh bare clone.
        std::fs::create_dir_all(&cas)
            .with_context(|| format!("failed to create {}", cas.display()))?;
        let status = Command::new("git")
            .args(["clone", "--bare", "--quiet", url])
            .arg(&cas)
            .status()
            .context("failed to run git clone")?;
        if !status.success() {
            // Clean up failed clone.
            let _ = std::fs::remove_dir_all(&cas);
            bail!("git clone failed for {url}");
        }
    }

    Ok(cas)
}

/// Resolve a git ref to a full commit hash.
fn resolve_commit(cas_dir: &Path, git_ref: Option<&str>) -> Result<String> {
    let ref_spec = git_ref.unwrap_or("HEAD");
    let output = Command::new("git")
        .args(["rev-parse", ref_spec])
        .current_dir(cas_dir)
        .output()
        .context("failed to run git rev-parse")?;
    if !output.status.success() {
        bail!(
            "git rev-parse {ref_spec} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Checkout a specific commit from a bare repo into a target directory.
fn checkout_to(cas_dir: &Path, commit: &str, dest: &Path) -> Result<()> {
    if dest.exists() {
        std::fs::remove_dir_all(dest)
            .with_context(|| format!("failed to remove existing {}", dest.display()))?;
    }
    std::fs::create_dir_all(dest)
        .with_context(|| format!("failed to create {}", dest.display()))?;

    // Use git archive to extract the tree without .git metadata.
    let output = Command::new("git")
        .args(["archive", "--format=tar", commit])
        .current_dir(cas_dir)
        .output()
        .context("git archive failed")?;
    if !output.status.success() {
        bail!(
            "git archive failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let status = Command::new("tar")
        .args(["xf", "-"])
        .current_dir(dest)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(ref mut stdin) = child.stdin {
                stdin.write_all(&output.stdout)?;
            }
            child.wait()
        })
        .context("tar extraction failed")?;

    if !status.success() {
        bail!("tar extraction failed for {commit}");
    }

    Ok(())
}

/// Try to read a version from a `.skill.toml` in the checked-out directory.
fn read_version(dep_dir: &Path) -> Option<String> {
    let config_path = dep_dir.join(".skill.toml");
    let content = std::fs::read_to_string(&config_path).ok()?;
    let config: crate::parser::SkillConfig = toml::from_str(&content).ok()?;
    config.skill.version
}

// ---------------------------------------------------------------------------
// SKILL.md case-mismatch detection
// ---------------------------------------------------------------------------

/// Search `dir` for a file whose name matches `SKILL.md` case-insensitively
/// but is **not** the canonical `SKILL.md`. Returns the first such filename
/// found, or `None` if there is no mismatch.
///
/// **Note**: On case-insensitive filesystems (e.g. macOS HFS+/APFS default),
/// the caller (`install_from_local`) resolves `SKILL.md` successfully even
/// when the on-disk name is `skill.md`, so this function is never reached.
/// Detection is therefore best-effort and only effective on case-sensitive
/// filesystems.
pub(crate) fn detect_skill_md_case_mismatch(dir: &Path) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.eq_ignore_ascii_case("SKILL.md") && name_str != "SKILL.md" {
            return Some(name_str.into_owned());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Public API: install
// ---------------------------------------------------------------------------

/// Install a skill from a git source into `.weave/deps/<name>/`.
///
/// Returns the locked package entry.
pub fn install(source: &str, project_root: &Path, cache_root: &Path) -> Result<LockedPackage> {
    let src = parse_source(source)?;
    let cas = ensure_cached(cache_root, &src.url)?;
    let commit = resolve_commit(&cas, src.git_ref.as_deref())?;

    let deps_dir = project_root.join(".weave").join("deps");
    let dest = deps_dir.join(&src.name);
    checkout_to(&cas, &commit, &dest)?;

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
    let mut lockfile = load_project_lockfile(project_root).unwrap_or(Lockfile {
        package: Vec::new(),
    });
    upsert_package(&mut lockfile, &pkg);
    save_lockfile(&lock_path, &lockfile)?;

    Ok(pkg)
}

// ---------------------------------------------------------------------------
// Public API: install_from_local
// ---------------------------------------------------------------------------

/// Install a skill from a local directory path into `.weave/deps/<name>/`.
///
/// The source directory is recursively copied (excluding `.git/`).
/// Returns the locked package entry with `source_kind = Local`.
pub fn install_from_local(source_path: &Path, project_root: &Path) -> Result<LockedPackage> {
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

    let deps_dir = project_root.join(".weave").join("deps");
    let dest = deps_dir.join(&name);

    // Guard against source/destination overlap.
    // Resolve dest without requiring it to exist: canonicalize the parent
    // (deps_dir) and append the name.  Also check whether source is inside
    // the project root to catch `weave install --path .`.
    let project_canonical = project_root
        .canonicalize()
        .with_context(|| format!("cannot resolve project root: {}", project_root.display()))?;
    {
        let dest_approx = if deps_dir.exists() {
            deps_dir.canonicalize()?.join(&name)
        } else {
            // deps/ doesn't exist yet.  Resolve .weave itself if it exists
            // (it might be a symlink) so the overlap check uses the real path.
            let weave_dir = project_root.join(".weave");
            if weave_dir.exists() {
                weave_dir.canonicalize()?.join("deps").join(&name)
            } else {
                project_canonical.join(".weave").join("deps").join(&name)
            }
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
    std::fs::create_dir_all(&deps_dir)
        .with_context(|| format!("failed to create {}", deps_dir.display()))?;
    let staging = deps_dir.join(format!(".{name}.staging.{}", std::process::id()));
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
    let mut lockfile = load_project_lockfile(project_root).unwrap_or(Lockfile {
        package: Vec::new(),
    });
    upsert_package(&mut lockfile, &pkg);
    save_lockfile(&lock_path, &lockfile)?;

    Ok(pkg)
}

/// Recursively copy a directory, skipping `.git/` subdirectories and symlinks.
///
/// Symlinks are skipped to prevent copying sensitive files that may be linked
/// from outside the skill directory (e.g., `secrets -> ~/.ssh/id_rsa`).
fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)
        .with_context(|| format!("failed to create {}", dest.display()))?;

    for entry in
        std::fs::read_dir(src).with_context(|| format!("failed to read {}", src.display()))?
    {
        let entry = entry?;
        let file_name = entry.file_name();
        let src_path = entry.path();
        let dest_path = dest.join(&file_name);

        // Skip .git directories.
        if file_name == ".git" {
            continue;
        }

        let file_type = entry.file_type()?;

        // Skip symlinks — following them could copy sensitive external files.
        if file_type.is_symlink() {
            continue;
        }

        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dest_path)?;
        } else if file_type.is_file() {
            std::fs::copy(&src_path, &dest_path).with_context(|| {
                format!(
                    "failed to copy {} -> {}",
                    src_path.display(),
                    dest_path.display()
                )
            })?;
        }
        // Skip special files (FIFOs, sockets, device nodes) — copying them
        // would block or fail.
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Public API: lock
// ---------------------------------------------------------------------------

/// Generate or regenerate the lockfile from the current `.weave/deps/` state.
///
/// For each dep directory that has a matching lockfile entry, keep it.
/// For new deps (no lockfile entry), attempt to discover the git remote.
pub fn lock(project_root: &Path) -> Result<Lockfile> {
    let deps_dir = project_root.join(".weave").join("deps");
    let lock_path = lockfile_path(project_root);

    let existing = load_project_lockfile(project_root).unwrap_or(Lockfile {
        package: Vec::new(),
    });

    // Index existing entries by name.
    let existing_map: BTreeMap<String, LockedPackage> = existing
        .package
        .into_iter()
        .map(|p| (p.name.clone(), p))
        .collect();

    let mut packages = Vec::new();

    if deps_dir.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(&deps_dir)
            .with_context(|| format!("failed to read {}", deps_dir.display()))?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(existing_pkg) = existing_map.get(&name) {
                // Keep existing lockfile entry, update version if changed.
                let mut pkg = existing_pkg.clone();
                pkg.version = read_version(&entry.path());
                packages.push(pkg);
            } else {
                // New dep without lockfile entry — record with unknown repo/commit.
                packages.push(LockedPackage {
                    name,
                    repo: String::new(),
                    commit: String::new(),
                    version: read_version(&entry.path()),
                    source_kind: SourceKind::default(),
                    requested_version: None,
                    resolved_ref: None,
                });
            }
        }
    }

    let lockfile = Lockfile { package: packages };
    save_lockfile(&lock_path, &lockfile)?;

    Ok(lockfile)
}

// ---------------------------------------------------------------------------
// Public API: update
// ---------------------------------------------------------------------------

/// Update one or all locked dependencies to their latest commit.
///
/// When `force` is false, dependencies with a `requested_version` (pinned)
/// are skipped. When `force` is true, pinned dependencies are re-fetched
/// and re-resolved from their pinned ref (not HEAD).
pub fn update(
    name: Option<&str>,
    project_root: &Path,
    cache_root: &Path,
    force: bool,
) -> Result<Vec<LockedPackage>> {
    let lock_path = lockfile_path(project_root);
    let mut lockfile =
        load_project_lockfile(project_root).context("no lockfile found — run `weave lock` first")?;

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

        // For pinned deps (with --force), re-resolve from the pinned ref.
        // For unpinned deps, resolve from HEAD.
        let resolve_ref = pkg.resolved_ref.as_deref();
        let new_commit = resolve_commit(&cas, resolve_ref)?;

        if new_commit != pkg.commit {
            let deps_dir = project_root.join(".weave").join("deps");
            let dest = deps_dir.join(&pkg.name);
            checkout_to(&cas, &new_commit, &dest)?;

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
// Public API: audit (in package_audit.rs)
// ---------------------------------------------------------------------------

#[path = "package_audit.rs"]
mod package_audit;
pub use package_audit::{AuditIssue, AuditResult, audit};

// ---------------------------------------------------------------------------
// Lockfile I/O
// ---------------------------------------------------------------------------

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
