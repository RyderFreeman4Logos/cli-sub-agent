//! Merge guard: deterministic gate preventing `gh pr merge` without pr-bot.
//!
//! When enabled, CSA writes a `gh` wrapper script to a guard directory and
//! prepends it to `PATH` in the tool subprocess environment.  The wrapper
//! intercepts `gh pr merge` commands and verifies a pr-bot completion marker
//! exists before forwarding to the real `gh` binary.  Beyond gate-keeping,
//! the wrapper automatically syncs the local default branch after a successful
//! merge (best-effort, non-fatal).
//!
//! This enforcement is environment-level (PATH injection), not prompt-level,
//! so it cannot be bypassed by the LLM rationalizing "this is simple enough
//! to merge directly".  The post-merge sync is a side-effect that keeps the
//! local checkout current across all CSA-invoked projects.
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

# Pass through --help / -h immediately — never block help output.
for arg in "$@"; do
  case "$arg" in
    --help|-h) ;; # detected below
    *) continue ;;
  esac
  # If we reach here, arg is --help or -h.
  # We need the real gh first, then exec.
  _NEED_HELP_PASSTHROUGH=true
  break
done

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

# Best-effort local sync of the default branch after a successful merge.
# Non-fatal: sync failure does NOT change the wrapper's exit code.
_post_merge_sync() {
  DEFAULT_BRANCH="$("${REAL_GH}" repo view --json defaultBranchRef -q '.defaultBranchRef.name' 2>/dev/null || echo main)"
  {
    git fetch origin "${DEFAULT_BRANCH}" &&
    git checkout "${DEFAULT_BRANCH}" &&
    git merge "origin/${DEFAULT_BRANCH}" --ff-only
  } 2>&1 || echo "NOTE: post-merge local sync failed (non-fatal). Run manually: git fetch origin && git checkout ${DEFAULT_BRANCH} && git merge origin/${DEFAULT_BRANCH} --ff-only" >&2
}

# Complete --help passthrough now that we have REAL_GH.
if [ "${_NEED_HELP_PASSTHROUGH:-}" = "true" ]; then
  exec "${REAL_GH}" "$@"
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
    MERGE_EXIT=0
    "${REAL_GH}" "${ARGS[@]}" || MERGE_EXIT=$?
    if [ "${MERGE_EXIT}" -eq 0 ]; then _post_merge_sync; fi
    exit ${MERGE_EXIT}
  fi
done

# Block -R/--repo (cross-repo merges bypass local guard context).
for arg in "$@"; do
  case "$arg" in
    -R|--repo|--repo=*)
      echo "BLOCKED: cross-repo merge (-R/--repo) is not supported by CSA merge guard." >&2
      exit 1
      ;;
  esac
done

# Extract PR number — scan args after "merge", skip flag values.
# Flags that take a following value (e.g. -t <subject>, --body <text>)
# are tracked so their values are not misidentified as PR numbers.
PR_NUMBER=""
SEEN_MERGE=false
HAS_NON_NUMERIC_POSITIONAL=false
SKIP_NEXT=false
for arg in "$@"; do
  if $SKIP_NEXT; then SKIP_NEXT=false; continue; fi
  case "$arg" in
    pr|merge) SEEN_MERGE=true ;;
    # Flags whose NEXT token is a value (not a PR number).
    -t|--subject|-b|--body|-F|--body-file|--merge-method|--match-head-commit|-H|--head|-A|--author-email)
      SKIP_NEXT=true ;;
    # Equals-form: value is embedded, just skip the whole token.
    --subject=*|--body=*|--body-file=*|--merge-method=*|--match-head-commit=*|--head=*|--author-email=*)
      ;;
    --*|-*) ;; # other flags (no value)
    *)
      if [ "${SEEN_MERGE}" = "true" ]; then
        if echo "$arg" | grep -qxE '[0-9]+'; then
          PR_NUMBER="$arg"
        else
          HAS_NON_NUMERIC_POSITIONAL=true
        fi
      fi
      ;;
  esac
done

# If we found non-numeric positionals but no numeric PR number, reject.
if [ -z "${PR_NUMBER}" ] && [ "${HAS_NON_NUMERIC_POSITIONAL}" = "true" ]; then
  echo "BLOCKED: merge guard only accepts numeric PR numbers." >&2
  echo "Use 'gh pr merge <NUMBER>' (e.g., gh pr merge 123)." >&2
  exit 1
fi

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
  # Exact SHA marker found — allow merge.
  MERGE_EXIT=0
  "${REAL_GH}" "$@" || MERGE_EXIT=$?
  if [ "${MERGE_EXIT}" -eq 0 ]; then
    # Audit event: emitted AFTER successful merge, BEFORE sync.
    EVENTS_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/cli-sub-agent/events"
    mkdir -p "${EVENTS_DIR}" 2>/dev/null || true
    AUDIT_TS="$(date -u +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || echo "unknown")"
    printf '{"event":"MergeCompleted","pr_number":%s,"head_sha":"%s","marker_path":"%s","timestamp":"%s"}\n' \
      "${PR_NUMBER}" "${HEAD_SHA}" "${MARKER_FILE}" "${AUDIT_TS}" \
      >> "${EVENTS_DIR}/merge-guard.jsonl" 2>/dev/null || true
    _post_merge_sync
  fi
  exit ${MERGE_EXIT}
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

/// Signature line present in every CSA-generated `gh` wrapper.
const WRAPPER_SIGNATURE: &str = "CSA merge guard";

/// Install the `gh` merge guard wrapper to the given directory.
///
/// Creates the directory if needed, writes the wrapper script, and sets it
/// executable.  Returns the full path to the installed `gh` wrapper.
///
/// If `dir/gh` already exists and is **not** a CSA wrapper (identified by the
/// `CSA merge guard` signature in its content), the function refuses to
/// overwrite and returns an error.  Reinstalling over an existing CSA wrapper
/// is allowed silently.
pub fn install_merge_guard(install_dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(install_dir)
        .with_context(|| format!("failed to create install dir: {}", install_dir.display()))?;

    let wrapper_path = install_dir.join("gh");

    // Protect against overwriting a real `gh` binary or unrelated script.
    if wrapper_path.exists() {
        let existing = fs::read_to_string(&wrapper_path).unwrap_or_default();
        if !existing.contains(WRAPPER_SIGNATURE) {
            anyhow::bail!(
                "Target {} already exists and is not a CSA wrapper. \
                 Use a dedicated directory like ~/.local/bin/csa-gh-guard",
                wrapper_path.display()
            );
        }
        // Existing file is a CSA wrapper — safe to overwrite (reinstall).
    }

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
    if content.contains(WRAPPER_SIGNATURE) {
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
mod tests;
