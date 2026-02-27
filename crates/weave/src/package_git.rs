//! Git operations and filesystem helpers for weave package management.
//!
//! Extracted from `package.rs` — contains CAS cache management, git clone/fetch,
//! commit resolution, checkout, directory copying, and SKILL.md detection.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

// ---------------------------------------------------------------------------
// CAS cache
// ---------------------------------------------------------------------------

/// Compute the CAS cache directory for a git URL.
///
/// Uses a simple deterministic path: `~/.cache/weave/git/<safe-name>/`
/// where safe-name is the URL with special chars replaced.
pub(super) fn cas_dir_for(cache_root: &Path, url: &str) -> PathBuf {
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
// Global package store
// ---------------------------------------------------------------------------

/// Return the global store root: `~/.local/share/weave/packages/`.
pub fn global_store_root() -> Result<PathBuf> {
    let base = directories::BaseDirs::new().context("cannot determine home directory")?;
    Ok(base.data_local_dir().join("weave").join("packages"))
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
pub(super) fn ensure_cached(cache_root: &Path, url: &str) -> Result<PathBuf> {
    let cas = cas_dir_for(cache_root, url);

    if cas.join("HEAD").is_file() {
        // Already cloned — fetch updates.
        // In a bare repo `git fetch --all` updates FETCH_HEAD but does NOT
        // advance local branch refs.  Use an explicit refspec so that
        // `resolve_commit(cas, None)` (which resolves HEAD → main) sees the
        // latest remote commit.
        let status = Command::new("git")
            .args(["fetch", "--quiet", "origin", "+refs/heads/*:refs/heads/*"])
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
pub(super) fn resolve_commit(cas_dir: &Path, git_ref: Option<&str>) -> Result<String> {
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
pub(super) fn checkout_to(cas_dir: &Path, commit: &str, dest: &Path) -> Result<()> {
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
pub(super) fn read_version(dep_dir: &Path) -> Option<String> {
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

/// Recursively copy a directory, skipping `.git/` subdirectories and symlinks.
///
/// Symlinks are skipped to prevent copying sensitive files that may be linked
/// from outside the skill directory (e.g., `secrets -> ~/.ssh/id_rsa`).
pub(super) fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
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
// Lockfile path helpers
// ---------------------------------------------------------------------------

/// Canonical lockfile path: `<project_root>/weave.lock`.
pub fn lockfile_path(project_root: &Path) -> PathBuf {
    project_root.join("weave.lock")
}

/// Legacy lockfile path: `<project_root>/.weave/lock.toml`.
pub(super) fn legacy_lockfile_path(project_root: &Path) -> PathBuf {
    project_root.join(".weave").join("lock.toml")
}

/// Find the lockfile, preferring the new path over the legacy one.
///
/// Returns `Some(path)` if a lockfile exists at either location, `None` otherwise.
/// Emits a deprecation warning when falling back to the legacy path.
pub fn find_lockfile(project_root: &Path) -> Option<PathBuf> {
    let new_path = lockfile_path(project_root);
    if new_path.is_file() {
        return Some(new_path);
    }
    let old_path = legacy_lockfile_path(project_root);
    if old_path.is_file() {
        tracing::warn!(
            "Using legacy .weave/lock.toml \u{2014} run `weave migrate` to upgrade to weave.lock"
        );
        return Some(old_path);
    }
    None
}

/// Load the project lockfile, searching both new and legacy paths.
pub fn load_project_lockfile(project_root: &Path) -> Result<super::Lockfile> {
    match find_lockfile(project_root) {
        Some(path) => super::load_lockfile(&path),
        None => bail!(
            "no lockfile found at {} or {}",
            lockfile_path(project_root).display(),
            legacy_lockfile_path(project_root).display()
        ),
    }
}
