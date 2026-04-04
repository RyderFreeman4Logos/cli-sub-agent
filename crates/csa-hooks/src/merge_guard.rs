//! Merge guard: deterministic gate preventing `gh pr merge` without pr-bot.
//!
//! When enabled, CSA writes a `gh` wrapper script to a guard directory and
//! prepends it to `PATH` in the tool subprocess environment.  The wrapper
//! intercepts `gh pr merge` commands and verifies a pr-bot completion marker
//! exists before forwarding to the real `gh` binary.
//!
//! This enforcement is environment-level (PATH injection), not prompt-level,
//! so it cannot be bypassed by the LLM rationalizing "this is simple enough
//! to merge directly".
//!
//! ## Activation
//!
//! The merge guard is always active in CSA subprocess environments.
//! `inject_merge_guard_env` unconditionally prepends the wrapper to `PATH`
//! for every tool subprocess, ensuring deterministic enforcement regardless
//! of configuration.
//!
//! The `is_merge_guard_enabled` helper is available for callers that need
//! an opt-out check (e.g. `csa hooks install`), but the subprocess injection
//! path does not consult it.
//!
//! ## Bypass
//!
//! The tool subprocess can include `--force-skip-pr-bot` in the `gh pr merge`
//! command to bypass the gate for emergencies.

use std::collections::HashMap;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::debug;

/// Directory name under CSA's data dir for guard scripts.
const GUARD_DIR_NAME: &str = "guards";

/// The `gh` wrapper script that checks for pr-bot markers.
///
/// Uses `CSA_REAL_GH` env var (set by CSA) to find the real `gh` binary,
/// avoiding PATH lookup loops.
const GH_WRAPPER: &str = r#"#!/usr/bin/env bash
# CSA merge guard: blocks `gh pr merge` unless pr-bot completed.
# Injected by CSA via PATH; bypass with --force-skip-pr-bot.
set -euo pipefail

# Forward non-merge commands immediately.
IS_MERGE=false
for arg in "$@"; do
  case "$arg" in
    merge) IS_MERGE=true; break ;;
    --*) ;; # skip flags
    -*) ;;  # skip short flags
    pr) ;;  # expected before merge
    *) break ;; # first positional that's not pr/merge → not a merge
  esac
done

REAL_GH="${CSA_REAL_GH:-}"
if [ -z "${REAL_GH}" ]; then
  # Fallback: find gh by stripping our guard dir AND user-installed
  # csa-gh-guard dirs from PATH to prevent recursive wrapper loops.
  GUARD_DIR="$(cd "$(dirname "$0")" && pwd)"
  CLEAN_PATH=""
  IFS=: read -ra _PATH_DIRS <<< "${PATH}"
  for _dir in "${_PATH_DIRS[@]}"; do
    [[ "$_dir" == "$GUARD_DIR" ]] && continue
    [[ "$_dir" == *csa-gh-guard* ]] && continue
    CLEAN_PATH="${CLEAN_PATH:+${CLEAN_PATH}:}${_dir}"
  done
  REAL_GH="$(PATH="${CLEAN_PATH}" command -v gh 2>/dev/null)" || true
fi

if [ -z "${REAL_GH}" ]; then
  echo "ERROR: CSA merge guard cannot find real gh binary." >&2
  exit 1
fi

if [ "${IS_MERGE}" != "true" ]; then
  exec "${REAL_GH}" "$@"
fi

# Block --auto / --enable-auto-merge unconditionally (even with --force-skip-pr-bot).
# Auto-merge bypasses the entire pr-bot workflow and is never safe.
for arg in "$@"; do
  case "$arg" in
    --auto|--enable-auto-merge)
      echo "BLOCKED: auto-merge is prohibited; use pr-bot workflow for merge." >&2
      exit 1
      ;;
  esac
done

# Check for bypass flag.
for arg in "$@"; do
  if [ "$arg" = "--force-skip-pr-bot" ]; then
    echo "WARNING: pr-bot gate bypassed via --force-skip-pr-bot" >&2
    # Remove the flag before forwarding (gh doesn't understand it).
    ARGS=()
    for a in "$@"; do
      [ "$a" != "--force-skip-pr-bot" ] && ARGS+=("$a")
    done
    exec "${REAL_GH}" "${ARGS[@]}"
  fi
done

# Block -R/--repo (cross-repo merges bypass local guard context).
for arg in "$@"; do
  case "$arg" in
    -R|--repo)
      echo "BLOCKED: cross-repo merge (-R/--repo) is not supported by CSA merge guard." >&2
      exit 1
      ;;
  esac
done

# Extract PR number from args (supports numeric and URL formats).
PR_NUMBER=""
for arg in "$@"; do
  case "$arg" in
    [0-9]*) PR_NUMBER="$arg"; break ;;
    https://*/pull/[0-9]*)
      PR_NUMBER="$(echo "$arg" | grep -oE '/pull/([0-9]+)' | grep -oE '[0-9]+')"
      [ -n "${PR_NUMBER}" ] && break
      ;;
  esac
done

if [ -z "${PR_NUMBER}" ]; then
  # No explicit PR number — try current branch's PR.
  PR_NUMBER="$("${REAL_GH}" pr view --json number -q '.number' 2>/dev/null)" || true
fi

if [ -z "${PR_NUMBER}" ]; then
  echo "BLOCKED: CSA merge guard cannot determine PR number." >&2
  echo "Use 'gh pr merge <NUMBER>' explicitly, or run /pr-bot first." >&2
  exit 1
fi

# Check marker — exact SHA match required.
REPO_SLUG="$("${REAL_GH}" repo view --json nameWithOwner -q '.nameWithOwner' 2>/dev/null | tr '/' '_')" || true
if [ -z "${REPO_SLUG}" ]; then
  REPO_SLUG="$(git remote get-url origin 2>/dev/null | sed -E 's#^(https?://[^/]+/|ssh://[^/]+/|[^:]+:)##; s/\.git$//' | tr '/' '_')"
fi

MARKER_DIR="${HOME}/.local/state/cli-sub-agent/pr-bot-markers/${REPO_SLUG}"

# Get the current head SHA of the PR for exact marker matching.
HEAD_SHA="$("${REAL_GH}" pr view "${PR_NUMBER}" --json headRefOid -q '.headRefOid' 2>/dev/null)" || true
if [ -z "${HEAD_SHA}" ]; then
  echo "BLOCKED: CSA merge guard cannot determine PR head SHA." >&2
  echo "Ensure 'gh' is authenticated and the PR exists." >&2
  exit 1
fi

MARKER_FILE="${MARKER_DIR}/${PR_NUMBER}-${HEAD_SHA}.done"

if [ -f "${MARKER_FILE}" ]; then
  # Exact SHA marker found — emit audit event and allow merge.
  EVENTS_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/cli-sub-agent/events"
  mkdir -p "${EVENTS_DIR}" 2>/dev/null || true
  AUDIT_TS="$(date -u +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || echo "unknown")"
  printf '{"event":"MergeCompleted","pr_number":%s,"head_sha":"%s","marker_path":"%s","timestamp":"%s"}\n' \
    "${PR_NUMBER}" "${HEAD_SHA}" "${MARKER_FILE}" "${AUDIT_TS}" \
    >> "${EVENTS_DIR}/merge-guard.jsonl" 2>/dev/null || true
  exec "${REAL_GH}" "$@"
else
  echo "BLOCKED: pr-bot has not completed for PR #${PR_NUMBER} at HEAD ${HEAD_SHA}." >&2
  echo "Run /pr-bot first, or add --force-skip-pr-bot to bypass." >&2
  if ls "${MARKER_DIR}/${PR_NUMBER}"-*.done 1>/dev/null 2>&1; then
    echo "NOTE: Stale markers exist for older commits. A new review is needed." >&2
  fi
  if [ -d "${MARKER_DIR}" ]; then
    echo "Marker directory: ${MARKER_DIR}" >&2
  fi
  exit 1
fi
"#;

/// Ensure the guard directory exists with an up-to-date `gh` wrapper.
///
/// Returns the guard directory path.  The caller should prepend this to
/// `PATH` in the tool subprocess environment.
pub fn ensure_guard_dir() -> Result<PathBuf> {
    let data_dir = csa_config::paths::state_dir()
        .context("cannot determine CSA state directory")?
        .join(GUARD_DIR_NAME);

    fs::create_dir_all(&data_dir)
        .with_context(|| format!("failed to create guard dir: {}", data_dir.display()))?;

    let wrapper_path = data_dir.join("gh");
    fs::write(&wrapper_path, GH_WRAPPER)
        .with_context(|| format!("failed to write gh wrapper: {}", wrapper_path.display()))?;
    #[cfg(unix)]
    fs::set_permissions(&wrapper_path, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("failed to chmod gh wrapper: {}", wrapper_path.display()))?;

    debug!(guard_dir = %data_dir.display(), "merge guard directory ready");
    Ok(data_dir)
}

/// Inject merge guard into a tool subprocess environment map.
///
/// Prepends the guard directory to `PATH` and sets `CSA_REAL_GH` so the
/// wrapper can find the real `gh` binary without PATH loops.
///
/// No-op if the guard directory cannot be set up (best-effort).
pub fn inject_merge_guard_env(env: &mut HashMap<String, String>) {
    let guard_dir = match ensure_guard_dir() {
        Ok(dir) => dir,
        Err(e) => {
            tracing::warn!("merge guard setup failed (best-effort skip): {e:#}");
            return;
        }
    };

    // Find the real `gh` binary BEFORE we modify PATH.
    // Skip our own guard directories (contain "csa-gh-guard" or match the
    // guard dir we just created) to avoid recursive wrapper loops.
    let real_gh = which::which_all("gh").ok().and_then(|iter| {
        let guard_dir_str = guard_dir.to_string_lossy();
        iter.into_iter().find(|p| {
            let s = p.to_string_lossy();
            !s.contains("csa-gh-guard") && !s.starts_with(guard_dir_str.as_ref())
        })
    });
    if let Some(real_gh) = real_gh {
        env.insert(
            "CSA_REAL_GH".to_string(),
            real_gh.to_string_lossy().into_owned(),
        );
    }

    // Prepend guard dir to PATH.
    let guard_dir_str = guard_dir.to_string_lossy().into_owned();
    let current_path = env
        .get("PATH")
        .cloned()
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();
    env.insert(
        "PATH".to_string(),
        format!("{guard_dir_str}:{current_path}"),
    );
}

/// Result of verifying a pr-bot marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkerStatus {
    /// Exact SHA marker found — merge allowed.
    Verified,
    /// No marker for current SHA, but stale markers exist for older commits.
    StaleMarkerExists,
    /// No marker at all for this PR.
    Missing,
}

/// Verify that a pr-bot completion marker exists for the exact PR + HEAD SHA.
///
/// This is the Rust-side equivalent of the shell check in the `gh` wrapper.
/// The `head_sha` parameter makes this testable without calling `gh`.
///
/// Marker path: `{marker_base_dir}/{repo_slug}/{pr_number}-{head_sha}.done`
pub fn verify_pr_bot_marker(
    marker_base_dir: &Path,
    repo_slug: &str,
    pr_number: u64,
    head_sha: &str,
) -> MarkerStatus {
    let marker_dir = marker_base_dir.join(repo_slug);
    let exact_marker = marker_dir.join(format!("{pr_number}-{head_sha}.done"));

    if exact_marker.is_file() {
        return MarkerStatus::Verified;
    }

    // Check for stale markers from previous commits.
    let pattern = format!("{pr_number}-");
    if marker_dir.is_dir()
        && let Ok(entries) = fs::read_dir(&marker_dir)
    {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with(&pattern) && name_str.ends_with(".done") {
                return MarkerStatus::StaleMarkerExists;
            }
        }
    }

    MarkerStatus::Missing
}

/// Return the raw `gh` wrapper script content.
///
/// Useful for the `install-merge-guard` CLI subcommand which writes the
/// wrapper to a user-chosen directory.
pub fn gh_wrapper_script() -> &'static str {
    GH_WRAPPER
}

/// Default install directory for the standalone merge guard wrapper.
pub fn default_install_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".local/bin/csa-gh-guard")
}

/// Install the `gh` merge guard wrapper to the given directory.
///
/// Creates the directory if needed, writes the wrapper script, and sets it
/// executable.  Returns the full path to the installed `gh` wrapper.
pub fn install_merge_guard(install_dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(install_dir)
        .with_context(|| format!("failed to create install dir: {}", install_dir.display()))?;

    let wrapper_path = install_dir.join("gh");
    fs::write(&wrapper_path, GH_WRAPPER)
        .with_context(|| format!("failed to write gh wrapper: {}", wrapper_path.display()))?;
    #[cfg(unix)]
    fs::set_permissions(&wrapper_path, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("failed to chmod gh wrapper: {}", wrapper_path.display()))?;

    debug!(path = %wrapper_path.display(), "merge guard installed");
    Ok(wrapper_path)
}

/// Check whether the merge guard wrapper appears in `PATH` ahead of the real `gh`.
///
/// Returns `Some(path)` to the guard wrapper if it is the first `gh` found,
/// `None` otherwise.
pub fn detect_installed_guard() -> Option<PathBuf> {
    let first_gh = which::which("gh").ok()?;
    // Check if the first `gh` in PATH is our wrapper (contains "CSA merge guard").
    let content = fs::read_to_string(&first_gh).ok()?;
    if content.contains("CSA merge guard") {
        Some(first_gh)
    } else {
        None
    }
}

/// Check if merge guard is enabled in hooks config.
///
/// Returns `true` unless explicitly disabled via `[hooks] merge_guard = false`.
pub fn is_merge_guard_enabled(hooks_path: Option<&Path>) -> bool {
    let Some(path) = hooks_path else {
        return true; // default: enabled
    };
    if !path.exists() {
        return true;
    }
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return true,
    };
    // Simple TOML check: look for `merge_guard = false`.
    // Full TOML parsing is overkill for a single boolean.
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "merge_guard = false" {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::sync::{LazyLock, Mutex};
    use tempfile::NamedTempFile;

    /// Process-wide lock for tests that mutate `XDG_STATE_HOME`.
    static GUARD_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    /// RAII guard that sets `XDG_STATE_HOME` to a temp path and restores
    /// the original value on drop — even if the test panics.
    struct ScopedXdgOverride {
        orig: Option<String>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl ScopedXdgOverride {
        fn new(tmp: &tempfile::TempDir) -> Self {
            let lock = GUARD_ENV_LOCK.lock().expect("env lock poisoned");
            let orig = std::env::var("XDG_STATE_HOME").ok();
            // SAFETY: test-scoped env mutation protected by GUARD_ENV_LOCK.
            unsafe {
                std::env::set_var("XDG_STATE_HOME", tmp.path().join("state").to_str().unwrap());
            }
            Self { orig, _lock: lock }
        }
    }

    impl Drop for ScopedXdgOverride {
        fn drop(&mut self) {
            // SAFETY: restoration of test-scoped env mutation (lock still held).
            unsafe {
                match &self.orig {
                    Some(v) => std::env::set_var("XDG_STATE_HOME", v),
                    None => std::env::remove_var("XDG_STATE_HOME"),
                }
            }
        }
    }

    #[test]
    fn test_ensure_guard_dir_creates_wrapper() {
        let tmp = tempfile::tempdir().unwrap();
        let _xdg = ScopedXdgOverride::new(&tmp);

        let dir = ensure_guard_dir().unwrap();
        let wrapper = dir.join("gh");
        assert!(wrapper.exists(), "gh wrapper should exist");
        assert!(
            wrapper.metadata().unwrap().permissions().mode() & 0o111 != 0,
            "gh wrapper should be executable"
        );
    }

    #[test]
    fn test_inject_merge_guard_env_sets_path() {
        let tmp = tempfile::tempdir().unwrap();
        let _xdg = ScopedXdgOverride::new(&tmp);

        let mut env = HashMap::new();
        env.insert("PATH".to_string(), "/usr/bin:/bin".to_string());
        inject_merge_guard_env(&mut env);

        let path = env.get("PATH").unwrap();
        assert!(
            path.contains("guards"),
            "PATH should contain guard dir: {path}"
        );
        assert!(
            path.ends_with("/usr/bin:/bin"),
            "original PATH should be preserved: {path}"
        );
    }

    #[test]
    fn test_inject_merge_guard_env_sets_real_gh() {
        let tmp = tempfile::tempdir().unwrap();
        let _xdg = ScopedXdgOverride::new(&tmp);

        let mut env = HashMap::new();
        inject_merge_guard_env(&mut env);
        // CSA_REAL_GH is set only if `gh` is installed.
        // In CI, gh may not be available, so we just check the key exists
        // when we know gh is installed.
        if which::which("gh").is_ok() {
            assert!(
                env.contains_key("CSA_REAL_GH"),
                "CSA_REAL_GH should be set when gh is installed"
            );
        }
    }

    #[test]
    fn test_is_merge_guard_enabled_default_true() {
        assert!(is_merge_guard_enabled(None));
    }

    #[test]
    fn test_is_merge_guard_enabled_nonexistent_file() {
        let path = Path::new("/nonexistent/hooks.toml");
        assert!(is_merge_guard_enabled(Some(path)));
    }

    #[test]
    fn test_is_merge_guard_disabled() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "[hooks]").unwrap();
        writeln!(f, "merge_guard = false").unwrap();
        f.flush().unwrap();
        assert!(!is_merge_guard_enabled(Some(f.path())));
    }

    #[test]
    fn test_is_merge_guard_enabled_explicit() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "[hooks]").unwrap();
        writeln!(f, "merge_guard = true").unwrap();
        f.flush().unwrap();
        assert!(is_merge_guard_enabled(Some(f.path())));
    }

    // --- verify_pr_bot_marker tests ---

    /// Helper: create a marker file in the temp directory.
    fn create_marker(base: &Path, repo: &str, filename: &str) {
        let dir = base.join(repo);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(filename), "").unwrap();
    }

    #[test]
    fn test_verify_marker_exact_sha_match() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        create_marker(base, "owner_repo", "42-abc123def.done");

        assert_eq!(
            verify_pr_bot_marker(base, "owner_repo", 42, "abc123def"),
            MarkerStatus::Verified
        );
    }

    #[test]
    fn test_verify_marker_wrong_sha_with_stale() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        // Old marker exists for a previous SHA.
        create_marker(base, "owner_repo", "42-oldsha999.done");

        assert_eq!(
            verify_pr_bot_marker(base, "owner_repo", 42, "newsha000"),
            MarkerStatus::StaleMarkerExists
        );
    }

    #[test]
    fn test_verify_marker_missing_no_markers() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        // No marker directory at all.
        assert_eq!(
            verify_pr_bot_marker(base, "owner_repo", 42, "abc123def"),
            MarkerStatus::Missing
        );
    }

    #[test]
    fn test_verify_marker_missing_dir_exists_but_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        fs::create_dir_all(base.join("owner_repo")).unwrap();

        assert_eq!(
            verify_pr_bot_marker(base, "owner_repo", 42, "abc123def"),
            MarkerStatus::Missing
        );
    }

    // --- GH wrapper auto-merge block tests ---
    //
    // These tests execute the wrapper script with a fake `gh` binary to verify
    // that `--auto` / `--enable-auto-merge` are unconditionally blocked.

    /// Create a minimal test environment with a fake `gh` and the wrapper script.
    /// Returns (guard_dir, fake_gh_path).
    fn setup_wrapper_env(tmp: &Path) -> (PathBuf, PathBuf) {
        let guard_dir = tmp.join("guard");
        fs::create_dir_all(&guard_dir).unwrap();

        // Write the wrapper script.
        let wrapper_path = guard_dir.join("gh");
        fs::write(&wrapper_path, GH_WRAPPER).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&wrapper_path, fs::Permissions::from_mode(0o755)).unwrap();

        // Create a fake `gh` that just prints "REAL_GH_CALLED" and exits 0.
        let fake_gh = tmp.join("fake_gh");
        fs::write(&fake_gh, "#!/bin/bash\necho REAL_GH_CALLED\n").unwrap();
        #[cfg(unix)]
        fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).unwrap();

        (guard_dir, fake_gh)
    }

    /// Run the wrapper with given args and return (exit_code, stdout, stderr).
    fn run_wrapper(guard_dir: &Path, fake_gh: &Path, args: &[&str]) -> (i32, String, String) {
        let wrapper = guard_dir.join("gh");
        let output = std::process::Command::new("bash")
            .arg(&wrapper)
            .args(args)
            .env("CSA_REAL_GH", fake_gh.to_str().unwrap())
            .output()
            .expect("failed to run wrapper");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
        )
    }

    #[test]
    fn test_wrapper_blocks_auto_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let (guard_dir, fake_gh) = setup_wrapper_env(tmp.path());

        let (code, _stdout, stderr) =
            run_wrapper(&guard_dir, &fake_gh, &["pr", "merge", "123", "--auto"]);
        assert_eq!(code, 1, "should exit 1 for --auto");
        assert!(
            stderr.contains("auto-merge is prohibited"),
            "stderr should contain prohibition message: {stderr}"
        );
    }

    #[test]
    fn test_wrapper_blocks_enable_auto_merge_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let (guard_dir, fake_gh) = setup_wrapper_env(tmp.path());

        let (code, _stdout, stderr) = run_wrapper(
            &guard_dir,
            &fake_gh,
            &["pr", "merge", "123", "--enable-auto-merge"],
        );
        assert_eq!(code, 1, "should exit 1 for --enable-auto-merge");
        assert!(
            stderr.contains("auto-merge is prohibited"),
            "stderr should contain prohibition message: {stderr}"
        );
    }

    #[test]
    fn test_wrapper_auto_flag_not_bypassed_by_force_skip() {
        let tmp = tempfile::tempdir().unwrap();
        let (guard_dir, fake_gh) = setup_wrapper_env(tmp.path());

        let (code, _stdout, stderr) = run_wrapper(
            &guard_dir,
            &fake_gh,
            &["pr", "merge", "123", "--auto", "--force-skip-pr-bot"],
        );
        assert_eq!(code, 1, "should exit 1 even with --force-skip-pr-bot");
        assert!(
            stderr.contains("auto-merge is prohibited"),
            "--force-skip-pr-bot must NOT bypass --auto block: {stderr}"
        );
    }

    #[test]
    fn test_wrapper_non_merge_command_passes_through() {
        let tmp = tempfile::tempdir().unwrap();
        let (guard_dir, fake_gh) = setup_wrapper_env(tmp.path());

        let (code, stdout, _stderr) = run_wrapper(&guard_dir, &fake_gh, &["pr", "view", "123"]);
        assert_eq!(code, 0, "non-merge commands should pass through");
        assert!(
            stdout.contains("REAL_GH_CALLED"),
            "should forward to real gh: {stdout}"
        );
    }

    #[test]
    fn test_wrapper_squash_merge_not_blocked_by_auto_check() {
        let tmp = tempfile::tempdir().unwrap();
        let (guard_dir, fake_gh) = setup_wrapper_env(tmp.path());

        // --squash without --auto should NOT be blocked by the auto-merge check.
        // It will proceed to the marker check (which will fail since there's no
        // marker), but the important thing is it's NOT blocked by the auto check.
        let (_code, _stdout, stderr) =
            run_wrapper(&guard_dir, &fake_gh, &["pr", "merge", "123", "--squash"]);
        assert!(
            !stderr.contains("auto-merge is prohibited"),
            "--squash should NOT trigger auto-merge block: {stderr}"
        );
    }

    #[test]
    fn test_wrapper_blocks_cross_repo_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let (guard_dir, fake_gh) = setup_wrapper_env(tmp.path());

        let (code, _stdout, stderr) = run_wrapper(
            &guard_dir,
            &fake_gh,
            &["pr", "merge", "123", "-R", "other/repo"],
        );
        assert_eq!(code, 1, "should exit 1 for -R flag");
        assert!(
            stderr.contains("cross-repo merge"),
            "stderr should mention cross-repo: {stderr}"
        );
    }

    #[test]
    fn test_wrapper_blocks_repo_long_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let (guard_dir, fake_gh) = setup_wrapper_env(tmp.path());

        let (code, _stdout, stderr) = run_wrapper(
            &guard_dir,
            &fake_gh,
            &["pr", "merge", "123", "--repo", "other/repo"],
        );
        assert_eq!(code, 1, "should exit 1 for --repo flag");
        assert!(
            stderr.contains("cross-repo merge"),
            "stderr should mention cross-repo: {stderr}"
        );
    }

    #[test]
    fn test_wrapper_extracts_pr_from_url() {
        let tmp = tempfile::tempdir().unwrap();
        let (guard_dir, fake_gh) = setup_wrapper_env(tmp.path());

        // The URL should be parsed to extract PR number 456.
        // It will proceed to marker check (which fails), but we verify
        // it correctly determines the PR number by checking the error message.
        let (_code, _stdout, stderr) = run_wrapper(
            &guard_dir,
            &fake_gh,
            &[
                "pr",
                "merge",
                "https://github.com/owner/repo/pull/456",
                "--squash",
            ],
        );
        // Should not say "cannot determine PR number" — that means URL parsing worked.
        assert!(
            !stderr.contains("cannot determine PR number"),
            "URL PR number extraction should work: {stderr}"
        );
    }

    #[test]
    fn test_verify_marker_different_pr_not_matched() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        // Marker for PR 99, not PR 42.
        create_marker(base, "owner_repo", "99-abc123def.done");

        assert_eq!(
            verify_pr_bot_marker(base, "owner_repo", 42, "abc123def"),
            MarkerStatus::Missing
        );
    }

    #[test]
    fn test_verify_marker_exact_takes_precedence_over_stale() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        // Both exact and stale markers exist.
        create_marker(base, "owner_repo", "42-oldsha999.done");
        create_marker(base, "owner_repo", "42-abc123def.done");

        assert_eq!(
            verify_pr_bot_marker(base, "owner_repo", 42, "abc123def"),
            MarkerStatus::Verified
        );
    }
}
