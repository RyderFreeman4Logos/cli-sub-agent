//! Cgroup v2 scope guard for systemd-based resource isolation.
//!
//! Wraps child tool processes in systemd transient scopes via `systemd-run
//! --user --scope`, applying `MemoryMax`, `MemorySwapMax`, and `TasksMax`
//! properties.  The [`CgroupScopeGuard`] owns the scope's lifecycle and stops
//! it on [`Drop`].
//!
//! # Recursive isolation
//!
//! Each `csa run` invocation creates an **independent** transient scope for
//! the tool binary it launches (e.g. `claude-code`, `codex`).  If that tool
//! then calls `csa run` recursively, the inner `csa` process — which runs
//! *inside* the parent scope — will create a **new, separate** scope for its
//! own tool child.  Because `systemd-run --scope` always creates a fresh
//! transient unit, scopes never nest: each has its own independent memory and
//! PID limits.  No environment variable handshake is required.
//!
//! ```text
//! csa (depth 0)
//!   └─ systemd scope "csa-claude-code-01J…" (MemoryMax=4096M)
//!        └─ claude-code
//!             └─ csa (depth 1)  ← runs inside parent scope, but…
//!                  └─ systemd scope "csa-codex-01J…" (MemoryMax=2048M)
//!                       └─ codex  ← gets its own independent limits
//! ```

use std::process::Command;

use anyhow::{Context, Result};
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// SandboxConfig
// ---------------------------------------------------------------------------

/// Resource limits to apply to a cgroup scope.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// Maximum physical memory in MB (`MemoryMax`).
    pub memory_max_mb: u64,
    /// Maximum swap in MB (`MemorySwapMax`).  `None` keeps the systemd
    /// default (unlimited swap).
    pub memory_swap_max_mb: Option<u64>,
    /// Maximum number of tasks/PIDs (`TasksMax`).  `None` keeps the systemd
    /// default (unlimited).
    pub pids_max: Option<u32>,
}

// ---------------------------------------------------------------------------
// Scope name helpers
// ---------------------------------------------------------------------------

/// Maximum length for a systemd unit name (bytes).
const MAX_SCOPE_NAME_LEN: usize = 256;

/// Build a deterministic scope unit name from tool name and session id.
///
/// Format: `csa-{tool_name}-{session_id_prefix}.scope`
/// Truncates `session_id` if the full name would exceed 256 bytes.
pub(crate) fn scope_unit_name(tool_name: &str, session_id: &str) -> String {
    // "csa-" + tool + "-" + session + ".scope"
    let prefix = format!("csa-{tool_name}-");
    let suffix = ".scope";
    let budget = MAX_SCOPE_NAME_LEN
        .saturating_sub(prefix.len())
        .saturating_sub(suffix.len());
    let truncated_id = &session_id[..session_id.len().min(budget)];
    format!("{prefix}{truncated_id}{suffix}")
}

// ---------------------------------------------------------------------------
// create_scope_command
// ---------------------------------------------------------------------------

/// Build a [`Command`] that launches a child process inside a systemd
/// transient scope with the given resource limits.
///
/// The returned `Command` targets `systemd-run` itself.  The caller must
/// append the actual tool binary and its arguments via
/// [`Command::arg`]/[`Command::args`] **after** this function returns.
///
/// # Example
///
/// ```no_run
/// use csa_resource::cgroup::{SandboxConfig, create_scope_command};
///
/// let cfg = SandboxConfig {
///     memory_max_mb: 4096,
///     memory_swap_max_mb: Some(0),
///     pids_max: Some(512),
/// };
/// let mut cmd = create_scope_command("claude-code", "01JEXAMPLE", &cfg);
/// cmd.arg("claude-code").arg("--yolo");
/// // let child = cmd.spawn()?;
/// ```
pub fn create_scope_command(tool_name: &str, session_id: &str, config: &SandboxConfig) -> Command {
    let unit = scope_unit_name(tool_name, session_id);

    let mut cmd = Command::new("systemd-run");
    cmd.args(["--user", "--scope", "--unit", &unit]);

    // Resource properties -------------------------------------------------
    cmd.args(["-p", &format!("MemoryMax={}M", config.memory_max_mb)]);

    if let Some(swap) = config.memory_swap_max_mb {
        cmd.args(["-p", &format!("MemorySwapMax={swap}M")]);
    }

    if let Some(pids) = config.pids_max {
        cmd.args(["-p", &format!("TasksMax={pids}")]);
    }

    // Separator: everything after "--" is the actual command the scope runs.
    cmd.arg("--");

    cmd
}

// ---------------------------------------------------------------------------
// CgroupScopeGuard (RAII)
// ---------------------------------------------------------------------------

/// RAII guard that stops a systemd transient scope on [`Drop`].
///
/// The guard does **not** own the child process; it only owns the scope
/// cleanup.  The caller spawns and manages the child via the [`Command`]
/// returned by [`create_scope_command`].
pub struct CgroupScopeGuard {
    scope_name: String,
}

impl CgroupScopeGuard {
    /// Create a guard for the given scope unit name.
    ///
    /// Call this *after* successfully spawning the child process inside the
    /// scope (i.e. after `cmd.spawn()` succeeds).
    pub fn new(tool_name: &str, session_id: &str) -> Self {
        let scope_name = scope_unit_name(tool_name, session_id);
        debug!(scope = %scope_name, "cgroup scope guard created");
        Self { scope_name }
    }

    /// The systemd unit name this guard will clean up.
    pub fn scope_name(&self) -> &str {
        &self.scope_name
    }

    /// Explicitly stop the scope.  Consumes the guard, preventing the
    /// [`Drop`] impl from running a second stop.
    pub fn stop(self) {
        self.stop_scope();
        // Prevent Drop from running again by consuming self (Drop still
        // runs, but `stop_scope` is idempotent for systemd).
    }

    /// Best-effort `systemctl --user stop`.
    fn stop_scope(&self) {
        debug!(scope = %self.scope_name, "stopping cgroup scope");
        let result = Command::new("systemctl")
            .args(["--user", "stop", &self.scope_name])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        match result {
            Ok(status) if status.success() => {
                debug!(scope = %self.scope_name, "scope stopped successfully");
            }
            Ok(status) => {
                // Non-zero exit is expected if the scope already exited.
                debug!(
                    scope = %self.scope_name,
                    code = status.code(),
                    "scope stop returned non-zero (may already be gone)"
                );
            }
            Err(e) => {
                warn!(
                    scope = %self.scope_name,
                    error = %e,
                    "failed to run systemctl stop"
                );
            }
        }
    }
}

impl Drop for CgroupScopeGuard {
    fn drop(&mut self) {
        self.stop_scope();
    }
}

// ---------------------------------------------------------------------------
// Orphan scope cleanup
// ---------------------------------------------------------------------------

/// Discovered orphan scope with its process count.
#[derive(Debug)]
pub struct OrphanScope {
    pub unit_name: String,
    pub active_pids: u32,
}

/// Find and stop cgroup scopes created by CSA that have no active processes.
///
/// Queries `systemctl --user list-units 'csa-*.scope'` and stops any whose
/// active PID count is zero.  Returns the list of scopes that were stopped.
///
/// Intended to be called from `csa gc` or `csa doctor`.
pub fn cleanup_orphan_scopes() -> Result<Vec<OrphanScope>> {
    let scopes = list_csa_scopes().context("failed to list csa scopes")?;
    let mut cleaned = Vec::new();

    for unit_name in scopes {
        let pids = scope_active_pids(&unit_name);
        // Only stop scopes confirmed to have 0 active PIDs.
        // If the query failed (None), leave the scope alone to avoid
        // accidentally killing a scope whose PID count is unknown.
        if pids == Some(0) {
            debug!(scope = %unit_name, "stopping orphan scope (0 active PIDs)");
            let _ = Command::new("systemctl")
                .args(["--user", "stop", &unit_name])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            cleaned.push(OrphanScope {
                unit_name,
                active_pids: 0,
            });
        }
    }

    Ok(cleaned)
}

/// List all running `csa-*.scope` user units.
fn list_csa_scopes() -> Result<Vec<String>> {
    let output = Command::new("systemctl")
        .args([
            "--user",
            "list-units",
            "csa-*.scope",
            "--no-legend",
            "--plain",
            "--no-pager",
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .context("systemctl not found or failed to execute")?;

    if !output.status.success() {
        // Empty list returns exit 0 on modern systemd; non-zero likely
        // means systemd user instance is unavailable.
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let units = stdout
        .lines()
        .filter_map(|line| {
            // Each line: UNIT LOAD ACTIVE SUB DESCRIPTION…
            let unit = line.split_whitespace().next()?;
            if unit.starts_with("csa-") && unit.ends_with(".scope") {
                Some(unit.to_string())
            } else {
                None
            }
        })
        .collect();

    Ok(units)
}

/// Query active PID count for a scope via `systemctl show`.
///
/// Returns `None` if the query fails (systemctl error, parse failure),
/// distinguishing "unknown" from "zero processes".
fn scope_active_pids(unit_name: &str) -> Option<u32> {
    let output = Command::new("systemctl")
        .args([
            "--user",
            "show",
            unit_name,
            "--property=TasksCurrent",
            "--value",
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let s = String::from_utf8_lossy(&output.stdout);
    s.trim().parse::<u32>().ok()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scope_unit_name_basic() {
        let name = scope_unit_name("claude-code", "01JABCDEF");
        assert_eq!(name, "csa-claude-code-01JABCDEF.scope");
    }

    #[test]
    fn test_scope_unit_name_truncation() {
        let long_id = "A".repeat(300);
        let name = scope_unit_name("x", &long_id);
        assert!(
            name.len() <= MAX_SCOPE_NAME_LEN,
            "scope name {} exceeds limit {}",
            name.len(),
            MAX_SCOPE_NAME_LEN,
        );
        assert!(name.starts_with("csa-x-"));
        assert!(name.ends_with(".scope"));
    }

    #[test]
    fn test_create_scope_command_full() {
        let cfg = SandboxConfig {
            memory_max_mb: 4096,
            memory_swap_max_mb: Some(0),
            pids_max: Some(512),
        };
        let cmd = create_scope_command("codex", "01JTEST", &cfg);
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();

        assert_eq!(cmd.get_program().to_string_lossy(), "systemd-run");
        assert!(args.contains(&"--user".to_string()));
        assert!(args.contains(&"--scope".to_string()));
        assert!(args.contains(&"csa-codex-01JTEST.scope".to_string()));
        assert!(args.contains(&"MemoryMax=4096M".to_string()));
        assert!(args.contains(&"MemorySwapMax=0M".to_string()));
        assert!(args.contains(&"TasksMax=512".to_string()));
        assert!(args.contains(&"--".to_string()));
    }

    #[test]
    fn test_create_scope_command_minimal() {
        let cfg = SandboxConfig {
            memory_max_mb: 1024,
            memory_swap_max_mb: None,
            pids_max: None,
        };
        let cmd = create_scope_command("gemini-cli", "01JXY", &cfg);
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();

        assert!(args.contains(&"MemoryMax=1024M".to_string()));
        // No swap or tasks properties when None.
        assert!(!args.iter().any(|a| a.contains("MemorySwapMax")));
        assert!(!args.iter().any(|a| a.contains("TasksMax")));
    }

    #[test]
    fn test_create_scope_command_separator_at_end() {
        let cfg = SandboxConfig {
            memory_max_mb: 512,
            memory_swap_max_mb: None,
            pids_max: None,
        };
        let cmd = create_scope_command("t", "s", &cfg);
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();

        // The "--" separator must be the last argument we set so that the
        // caller can append the tool binary after it.
        assert_eq!(args.last().unwrap(), "--");
    }

    #[test]
    fn test_cgroup_scope_guard_name() {
        let guard = CgroupScopeGuard::new("claude-code", "01JGUARD");
        assert_eq!(guard.scope_name(), "csa-claude-code-01JGUARD.scope");
        // Drop will attempt `systemctl stop` which will fail silently in CI
        // (no systemd user session). That's fine — it's best-effort.
    }
}
