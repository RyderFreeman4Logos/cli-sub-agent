//! Weave git-native package management.
//!
//! Skills are distributed as git repositories. Weave clones them into a
//! content-addressable cache (`~/.cache/weave/git/<url-hash>/`) and checks
//! out the requested revision into `.weave/deps/<name>/`.
//!
//! Commands:
//! - `install <source>` — clone/fetch + checkout into deps
//! - `lock` — snapshot current deps into `.weave/lock.toml`
//! - `update [name]` — fetch latest and re-lock
//! - `audit` — verify lockfile consistency

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Lockfile types
// ---------------------------------------------------------------------------

/// Root structure of `.weave/lock.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Lockfile {
    #[serde(default)]
    pub package: Vec<LockedPackage>,
}

/// A single locked dependency.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LockedPackage {
    pub name: String,
    pub repo: String,
    pub commit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
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
    };

    // Update the lockfile with this package.
    let lockfile_path = project_root.join(".weave").join("lock.toml");
    let mut lockfile = load_lockfile(&lockfile_path).unwrap_or(Lockfile {
        package: Vec::new(),
    });
    upsert_package(&mut lockfile, &pkg);
    save_lockfile(&lockfile_path, &lockfile)?;

    Ok(pkg)
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
    let lockfile_path = project_root.join(".weave").join("lock.toml");

    let existing = load_lockfile(&lockfile_path).unwrap_or(Lockfile {
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
                });
            }
        }
    }

    let lockfile = Lockfile { package: packages };
    save_lockfile(&lockfile_path, &lockfile)?;

    Ok(lockfile)
}

// ---------------------------------------------------------------------------
// Public API: update
// ---------------------------------------------------------------------------

/// Update one or all locked dependencies to their latest commit.
pub fn update(
    name: Option<&str>,
    project_root: &Path,
    cache_root: &Path,
) -> Result<Vec<LockedPackage>> {
    let lockfile_path = project_root.join(".weave").join("lock.toml");
    let mut lockfile =
        load_lockfile(&lockfile_path).context("no lockfile found — run `weave lock` first")?;

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
        if pkg.repo.is_empty() {
            continue; // Skip entries without a known repo.
        }

        let cas = ensure_cached(cache_root, &pkg.repo)?;
        let new_commit = resolve_commit(&cas, None)?;

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

    save_lockfile(&lockfile_path, &lockfile)?;
    Ok(updated)
}

// ---------------------------------------------------------------------------
// Public API: audit
// ---------------------------------------------------------------------------

/// Audit result for a single package.
#[derive(Debug)]
pub struct AuditResult {
    pub name: String,
    pub issues: Vec<AuditIssue>,
}

/// A single audit issue.
#[derive(Debug)]
pub enum AuditIssue {
    /// Dependency in lockfile but missing from `.weave/deps/`.
    MissingFromDeps,
    /// Dependency in `.weave/deps/` but not in lockfile.
    MissingFromLockfile,
    /// Empty repo URL in lockfile — not installed via weave.
    UnknownRepo,
    /// SKILL.md not found in dependency directory.
    MissingSkillMd,
    /// Symlink target does not exist.
    BrokenSymlink {
        /// Path of the broken symlink.
        path: PathBuf,
        /// Target the symlink points to.
        target: PathBuf,
    },
}

impl std::fmt::Display for AuditIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingFromDeps => write!(f, "locked but missing from .weave/deps/"),
            Self::MissingFromLockfile => write!(f, "present in deps but not in lockfile"),
            Self::UnknownRepo => write!(f, "lockfile entry has no repo URL"),
            Self::MissingSkillMd => write!(f, "no SKILL.md found"),
            Self::BrokenSymlink { path, target } => {
                write!(
                    f,
                    "broken symlink: {} -> {}",
                    path.display(),
                    target.display()
                )
            }
        }
    }
}

/// Audit installed skills for consistency issues.
pub fn audit(project_root: &Path) -> Result<Vec<AuditResult>> {
    let deps_dir = project_root.join(".weave").join("deps");
    let lockfile_path = project_root.join(".weave").join("lock.toml");

    let lockfile = load_lockfile(&lockfile_path).unwrap_or(Lockfile {
        package: Vec::new(),
    });

    let locked_names: BTreeMap<String, &LockedPackage> = lockfile
        .package
        .iter()
        .map(|p| (p.name.clone(), p))
        .collect();

    let mut results = Vec::new();

    // Check each locked package.
    for pkg in &lockfile.package {
        let mut issues = Vec::new();
        let dep_path = deps_dir.join(&pkg.name);

        if !dep_path.is_dir() {
            issues.push(AuditIssue::MissingFromDeps);
        } else if !dep_path.join("SKILL.md").is_file() {
            issues.push(AuditIssue::MissingSkillMd);
        }

        if pkg.repo.is_empty() {
            issues.push(AuditIssue::UnknownRepo);
        }

        if !issues.is_empty() {
            results.push(AuditResult {
                name: pkg.name.clone(),
                issues,
            });
        }
    }

    // Check for deps not in lockfile.
    if deps_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&deps_dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                if entry.path().is_dir() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !locked_names.contains_key(&name) {
                        results.push(AuditResult {
                            name,
                            issues: vec![AuditIssue::MissingFromLockfile],
                        });
                    }
                }
            }
        }
    }

    Ok(results)
}

// ---------------------------------------------------------------------------
// Public API: check (symlink health)
// ---------------------------------------------------------------------------

/// Default directories to scan for broken symlinks.
pub const DEFAULT_CHECK_DIRS: &[&str] = &[
    ".claude/skills",
    ".codex/skills",
    ".agents/skills",
    ".gemini/skills",
];

/// Result of checking a single directory for broken symlinks.
#[derive(Debug)]
pub struct CheckResult {
    /// Directory that was scanned.
    pub dir: PathBuf,
    /// Broken symlinks found.
    pub issues: Vec<AuditIssue>,
    /// Number of symlinks that were removed (when fix=true).
    pub fixed: usize,
    /// Number of symlinks that could not be removed (permission errors, etc.).
    pub fix_failures: usize,
}

/// Scan directories for broken symlinks.
///
/// When `fix` is true, broken symlinks are removed and the count is returned
/// in `CheckResult::fixed`. Only actual symlinks are removed — regular files
/// and directories are never touched.
pub fn check_symlinks(
    project_root: &Path,
    dirs: &[PathBuf],
    fix: bool,
) -> Result<Vec<CheckResult>> {
    let mut results = Vec::new();

    for dir in dirs {
        let abs_dir = if dir.is_absolute() {
            dir.clone()
        } else {
            project_root.join(dir)
        };

        if !abs_dir.is_dir() {
            continue;
        }

        let mut issues = Vec::new();
        let mut fixed = 0;
        let mut fix_failures = 0;
        // Track visited inodes to detect symlink cycles.
        let mut visited = HashSet::new();

        let entries = std::fs::read_dir(&abs_dir)
            .with_context(|| format!("failed to read {}", abs_dir.display()))?;

        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();

            // Use symlink_metadata to inspect the link itself, not its target.
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };

            if !meta.file_type().is_symlink() {
                continue;
            }

            // Cycle detection via inode.
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let inode = meta.ino();
                if !visited.insert(inode) {
                    continue; // Already seen this inode.
                }
            }

            let target = match std::fs::read_link(&path) {
                Ok(t) => t,
                Err(_) => continue,
            };

            // Resolve relative targets against the symlink's parent directory.
            let resolved = if target.is_absolute() {
                target.clone()
            } else {
                abs_dir.join(&target)
            };

            // Check if target exists (without following further symlinks).
            if !resolved.exists() {
                issues.push(AuditIssue::BrokenSymlink {
                    path: path.clone(),
                    target: target.clone(),
                });

                if fix {
                    // Only remove the symlink itself, never follow it.
                    if let Ok(m) = std::fs::symlink_metadata(&path) {
                        if m.file_type().is_symlink() {
                            match std::fs::remove_file(&path) {
                                Ok(()) => fixed += 1,
                                Err(_) => fix_failures += 1,
                            }
                        }
                    }
                }
            }
        }

        if !issues.is_empty() || fixed > 0 || fix_failures > 0 {
            results.push(CheckResult {
                dir: abs_dir,
                issues,
                fixed,
                fix_failures,
            });
        }
    }

    Ok(results)
}

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
