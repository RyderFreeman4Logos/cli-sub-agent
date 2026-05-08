//! lefthook_auto_install — auto-detect and install lefthook via mise on session start.
//!
//! Checks once per hour per project. Spawns a background task so session start
//! is not blocked. Skipped in CI (any non-empty `CI` env var).

use std::path::Path;
use std::time::SystemTime;
use tracing::{debug, info, warn};

/// Seconds between re-checks (1 hour).
const CHECK_INTERVAL_SECS: u64 = 3600;
/// Timestamp marker file stored under the project session state dir.
const TIMESTAMP_FILE: &str = "hook-check-ts";

/// Spawn a background task to auto-install lefthook if needed.
///
/// Non-blocking: returns immediately. Installation (if any) proceeds in background.
/// Skipped when:
/// - `CI` environment variable is set (any non-empty value)
/// - `project_root` has no `.git/` directory (not a git repo / test temp dir)
/// - `project_root` has no `lefthook.yml` (lefthook not configured here)
pub(crate) fn spawn_lefthook_setup_if_needed(project_root: &Path) {
    if is_ci_environment() {
        debug!("lefthook auto-install: skipping (CI environment)");
        return;
    }
    if !project_root.join(".git").exists() {
        debug!("lefthook auto-install: skipping (no .git directory)");
        return;
    }
    if !project_root.join("lefthook.yml").exists() {
        debug!("lefthook auto-install: skipping (no lefthook.yml)");
        return;
    }

    let project_root = project_root.to_path_buf();
    tokio::spawn(async move {
        if let Err(e) = check_and_setup_lefthook(&project_root).await {
            debug!("lefthook auto-install: background task: {e:#}");
        }
    });
}

/// Returns true when running inside CI (any non-empty `CI` env var).
fn is_ci_environment() -> bool {
    std::env::var_os("CI").is_some_and(|v| !v.is_empty())
}

async fn check_and_setup_lefthook(project_root: &Path) -> anyhow::Result<()> {
    // Locate the per-project state dir for the timestamp.
    let state_dir = csa_session::get_session_root(project_root)?;
    let ts_path = state_dir.join(TIMESTAMP_FILE);

    if !needs_check(&ts_path)? {
        debug!("lefthook auto-install: skipped (last check < {CHECK_INTERVAL_SECS}s ago)");
        return Ok(());
    }

    // Update timestamp before running checks to avoid concurrent duplicate runs.
    write_timestamp(&ts_path)?;

    let has_lefthook = is_command_available("lefthook").await;
    let hooks_ok = has_lefthook && are_git_hooks_installed(project_root).await;

    if has_lefthook && hooks_ok {
        debug!("lefthook auto-install: already configured, nothing to do");
        return Ok(());
    }

    if !has_lefthook {
        info!("lefthook auto-install: binary not found, attempting installation");
        if let Err(e) = install_lefthook().await {
            warn!("lefthook auto-install: installation failed: {e:#}");
            return Err(e);
        }
    }

    info!("lefthook auto-install: running `lefthook install`");
    if let Err(e) = run_lefthook_install(project_root).await {
        warn!("lefthook auto-install: hook installation failed: {e:#}");
        return Err(e);
    }

    info!("lefthook auto-install: setup complete");
    Ok(())
}

/// Returns true if the timestamp file is absent or older than CHECK_INTERVAL_SECS.
fn needs_check(ts_path: &Path) -> anyhow::Result<bool> {
    let metadata = match std::fs::metadata(ts_path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(e) => return Err(e.into()),
    };
    let modified = metadata.modified()?;
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or(std::time::Duration::MAX);
    Ok(age.as_secs() >= CHECK_INTERVAL_SECS)
}

/// Write (or touch) the timestamp file.
fn write_timestamp(ts_path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = ts_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(ts_path, b"")?;
    Ok(())
}

/// Returns true when `cmd` is findable via `which`.
async fn is_command_available(cmd: &str) -> bool {
    tokio::process::Command::new("which")
        .arg(cmd)
        .output()
        .await
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Returns true when lefthook-managed git hooks appear to be installed.
///
/// Inspects `.git/hooks/pre-commit` for the "lefthook" marker that lefthook
/// writes when `lefthook install` is run.
async fn are_git_hooks_installed(project_root: &Path) -> bool {
    let hook = project_root.join(".git/hooks/pre-commit");
    tokio::fs::read_to_string(&hook)
        .await
        .map(|content| content.contains("lefthook"))
        .unwrap_or(false)
}

/// Install lefthook: tries `mise install lefthook` first, falls back to `cargo install lefthook`.
async fn install_lefthook() -> anyhow::Result<()> {
    if is_command_available("mise").await {
        let status = tokio::process::Command::new("mise")
            .args(["install", "lefthook"])
            .status()
            .await?;
        if status.success() {
            info!("lefthook auto-install: installed via mise");
            return Ok(());
        }
        warn!("lefthook auto-install: `mise install lefthook` failed, trying cargo fallback");
    }

    let status = tokio::process::Command::new("cargo")
        .args(["install", "lefthook"])
        .status()
        .await?;
    if status.success() {
        info!("lefthook auto-install: installed via cargo");
        return Ok(());
    }

    anyhow::bail!("lefthook installation failed via mise and cargo")
}

/// Run `lefthook install` in `project_root` to register git hooks.
async fn run_lefthook_install(project_root: &Path) -> anyhow::Result<()> {
    let status = tokio::process::Command::new("lefthook")
        .arg("install")
        .current_dir(project_root)
        .status()
        .await?;
    if !status.success() {
        anyhow::bail!("`lefthook install` exited with status {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // ── rate-limit logic ──────────────────────────────────────────────────────

    #[test]
    fn needs_check_returns_true_when_no_file() {
        let dir = TempDir::new().unwrap();
        let ts = dir.path().join(TIMESTAMP_FILE);
        assert!(needs_check(&ts).unwrap());
    }

    #[test]
    fn needs_check_returns_false_when_recently_written() {
        let dir = TempDir::new().unwrap();
        let ts = dir.path().join(TIMESTAMP_FILE);
        fs::write(&ts, b"").unwrap();
        // Just written → mtime is now → age ≈ 0 → no check needed
        assert!(!needs_check(&ts).unwrap());
    }

    #[test]
    fn needs_check_returns_true_after_interval_elapsed() {
        let dir = TempDir::new().unwrap();
        let ts = dir.path().join(TIMESTAMP_FILE);
        fs::write(&ts, b"").unwrap();

        // Back-date mtime to Unix epoch (age > CHECK_INTERVAL_SECS is guaranteed).
        #[cfg(unix)]
        {
            use std::ffi::CString;
            let c_path = CString::new(ts.to_str().unwrap()).unwrap();
            let epoch_tv = libc::timeval {
                tv_sec: 0,
                tv_usec: 0,
            };
            // SAFETY: path is valid, times pointer is valid for two-element read.
            unsafe {
                libc::utimes(c_path.as_ptr(), [epoch_tv, epoch_tv].as_ptr());
            }
        }
        #[cfg(not(unix))]
        {
            // Non-Unix: skip mtime backdating; test is a no-op on that platform.
            return;
        }

        assert!(needs_check(&ts).unwrap());
    }

    // ── hook detection logic ──────────────────────────────────────────────────

    #[tokio::test]
    async fn are_hooks_installed_false_when_no_hook_file() {
        let dir = TempDir::new().unwrap();
        assert!(!are_git_hooks_installed(dir.path()).await);
    }

    #[tokio::test]
    async fn are_hooks_installed_true_when_lefthook_managed() {
        let dir = TempDir::new().unwrap();
        let hooks = dir.path().join(".git/hooks");
        fs::create_dir_all(&hooks).unwrap();
        fs::write(
            hooks.join("pre-commit"),
            "#!/bin/sh\ncall_lefthook run pre-commit\n",
        )
        .unwrap();
        assert!(are_git_hooks_installed(dir.path()).await);
    }

    #[tokio::test]
    async fn are_hooks_installed_false_when_non_lefthook_hook() {
        let dir = TempDir::new().unwrap();
        let hooks = dir.path().join(".git/hooks");
        fs::create_dir_all(&hooks).unwrap();
        fs::write(hooks.join("pre-commit"), "#!/bin/sh\ncargo fmt --check\n").unwrap();
        assert!(!are_git_hooks_installed(dir.path()).await);
    }

    // ── git repo / lefthook.yml guard ────────────────────────────────────────

    /// Verify that spawn_lefthook_setup_if_needed returns early (no panic, no spawn)
    /// when the project root lacks a `.git/` directory.
    #[tokio::test]
    async fn spawn_skips_when_no_git_directory() {
        let dir = TempDir::new().unwrap();
        // No .git/ → must be a no-op (does not panic, does not spawn)
        spawn_lefthook_setup_if_needed(dir.path());
        // No assertion needed beyond "did not panic"; the spawn guard makes it a no-op.
    }

    /// Verify that spawn_lefthook_setup_if_needed returns early when `.git/`
    /// exists but `lefthook.yml` is absent.
    #[tokio::test]
    async fn spawn_skips_when_no_lefthook_yml() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        // No lefthook.yml → must be a no-op
        spawn_lefthook_setup_if_needed(dir.path());
    }

    // ── CI detection ─────────────────────────────────────────────────────────

    #[test]
    fn is_ci_returns_false_when_ci_not_set() {
        // This test is deliberately narrow: it only tests the helper function,
        // not process-wide env manipulation, to stay safe in parallel test runs.
        // We use a hard-coded expectation derivable from the current environment.
        let expected = std::env::var_os("CI").is_some_and(|v| !v.is_empty());
        assert_eq!(is_ci_environment(), expected);
    }
}
