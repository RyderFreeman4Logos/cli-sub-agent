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
//! ## Configuration
//!
//! ```toml
//! # .csa/config.toml or ~/.config/cli-sub-agent/config.toml
//! [hooks]
//! merge_guard = true   # default: true
//! ```
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
  # Fallback: find gh by stripping our guard dir from PATH.
  GUARD_DIR="$(cd "$(dirname "$0")" && pwd)"
  CLEAN_PATH="$(echo "${PATH}" | tr ':' '\n' | grep -v "^${GUARD_DIR}$" | tr '\n' ':')"
  REAL_GH="$(PATH="${CLEAN_PATH}" command -v gh 2>/dev/null)" || true
fi

if [ -z "${REAL_GH}" ]; then
  echo "ERROR: CSA merge guard cannot find real gh binary." >&2
  exit 1
fi

if [ "${IS_MERGE}" != "true" ]; then
  exec "${REAL_GH}" "$@"
fi

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

# Extract PR number from args.
PR_NUMBER=""
for arg in "$@"; do
  case "$arg" in
    [0-9]*) PR_NUMBER="$arg"; break ;;
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

# Check marker.
REPO_SLUG="$("${REAL_GH}" repo view --json nameWithOwner -q '.nameWithOwner' 2>/dev/null | tr '/' '_')" || true
if [ -z "${REPO_SLUG}" ]; then
  REPO_SLUG="$(git remote get-url origin 2>/dev/null | sed -E 's#^(https?://[^/]+/|ssh://[^/]+/|[^:]+:)##; s/\.git$//' | tr '/' '_')"
fi

MARKER_DIR="${HOME}/.local/state/cli-sub-agent/pr-bot-markers/${REPO_SLUG}"

if ls "${MARKER_DIR}/${PR_NUMBER}"-*.done 1>/dev/null 2>&1; then
  # Marker found — allow merge.
  exec "${REAL_GH}" "$@"
else
  echo "BLOCKED: pr-bot has not completed for PR #${PR_NUMBER}." >&2
  echo "Run /pr-bot first, or add --force-skip-pr-bot to bypass." >&2
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
    if let Ok(real_gh) = which::which("gh") {
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
}
