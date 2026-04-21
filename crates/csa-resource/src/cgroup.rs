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

use std::collections::HashMap;
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
/// SIGKILL is the only signal we currently treat as an OOM fallback hint.
const SIGKILL: i32 = 9;

/// Build a deterministic scope unit name from tool name and session id.
///
/// Format: `csa-{tool_name}-{session_id_prefix}.scope`
/// Truncates `session_id` if the full name would exceed 256 bytes.
pub fn scope_unit_name(tool_name: &str, session_id: &str) -> String {
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
fn populate_scope_command(
    cmd: &mut Command,
    tool_name: &str,
    session_id: &str,
    config: &SandboxConfig,
) {
    let unit = scope_unit_name(tool_name, session_id);

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
}

pub fn create_scope_command(tool_name: &str, session_id: &str, config: &SandboxConfig) -> Command {
    let mut cmd = Command::new("systemd-run");
    populate_scope_command(&mut cmd, tool_name, session_id, config);
    cmd
}

/// Build a `systemd-run` scope command for callers that also apply an
/// environment block via [`Command::env`].
///
/// The environment must NOT be copied onto the `systemd-run` command line,
/// because that would expose secrets such as API keys via `ps` / `/proc`.
pub fn create_scope_command_with_env(
    tool_name: &str,
    session_id: &str,
    config: &SandboxConfig,
    _env: &HashMap<String, String>,
) -> Command {
    let mut cmd = Command::new("systemd-run");
    populate_scope_command(&mut cmd, tool_name, session_id, config);
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
    configured_memory_max_mb: u64,
    configured_memory_swap_max_mb: Option<u64>,
    collect_mode_persists_failures: bool,
}

#[derive(Debug, Default)]
struct ScopeProperties {
    load_state: Option<String>,
    result: Option<String>,
    memory_peak_bytes: Option<u64>,
    memory_max_bytes: Option<u64>,
    memory_swap_max_bytes: Option<u64>,
}

impl ScopeProperties {
    fn is_not_found(&self) -> bool {
        self.load_state.as_deref() == Some("not-found")
    }

    fn is_empty(&self) -> bool {
        self.load_state.is_none()
            && self.result.is_none()
            && self.memory_peak_bytes.is_none()
            && self.memory_max_bytes.is_none()
            && self.memory_swap_max_bytes.is_none()
    }
}

impl CgroupScopeGuard {
    /// Create a guard for the given scope unit name.
    ///
    /// Call this *after* successfully spawning the child process inside the
    /// scope (i.e. after `cmd.spawn()` succeeds).
    pub fn new(tool_name: &str, session_id: &str, config: &SandboxConfig) -> Self {
        let scope_name = scope_unit_name(tool_name, session_id);
        debug!(scope = %scope_name, "cgroup scope guard created");
        Self {
            scope_name,
            configured_memory_max_mb: config.memory_max_mb,
            configured_memory_swap_max_mb: config.memory_swap_max_mb,
            collect_mode_persists_failures: true,
        }
    }

    /// The systemd unit name this guard will clean up.
    pub fn scope_name(&self) -> &str {
        &self.scope_name
    }

    /// Check if the OOM killer was triggered inside this cgroup scope.
    ///
    /// Queries `systemctl --user show <scope> --property=Result` for
    /// `oom-kill`, which systemd sets when the scope terminates due to
    /// memory limit enforcement.  Must be called **before** [`Self::stop`]
    /// or [`Drop`], as the property is unavailable after the scope is gone.
    ///
    /// Returns `false` if the scope has already been cleaned up, unless the
    /// caller separately observed `SIGKILL` and uses
    /// [`Self::check_oom_killed_with_signal`].
    pub fn check_oom_killed(&self) -> bool {
        self.check_oom_killed_with_signal(None)
    }

    /// Check whether this scope was OOM-killed, falling back to SIGKILL when
    /// systemd already garbage-collected the failed scope state.
    pub fn check_oom_killed_with_signal(&self, exit_signal: Option<i32>) -> bool {
        match self.query_scope_properties() {
            Some(properties) if properties.result.as_deref() == Some("oom-kill") => true,
            Some(properties) if properties.is_empty() => self.should_assume_oom(exit_signal),
            Some(_) => false,
            None => self.should_assume_oom(exit_signal),
        }
    }

    /// Query peak memory usage (in MB) for this scope.
    ///
    /// Uses `systemctl --user show <scope> --property=MemoryPeak`.
    /// Returns `None` if the scope is gone or the query fails.
    pub fn memory_peak_mb(&self) -> Option<u64> {
        self.query_scope_properties()
            .and_then(|properties| properties.memory_peak_bytes)
            .map(bytes_to_mb)
    }

    /// Query current memory usage in bytes for this scope.
    ///
    /// Uses `systemctl --user show <scope> --property=MemoryCurrent`.
    /// Returns `None` if the scope is gone or the query fails.
    pub fn memory_current_bytes(&self) -> Option<u64> {
        let output = Command::new("systemctl")
            .args([
                "--user",
                "show",
                &self.scope_name,
                "--property=MemoryCurrent",
                "--value",
            ])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let value = String::from_utf8_lossy(&output.stdout);
        let trimmed = value.trim();
        if trimmed == "infinity" || trimmed.is_empty() {
            return None;
        }
        trimmed.parse::<u64>().ok()
    }

    /// Query configured memory limit (in MB) for this scope.
    ///
    /// Uses `systemctl --user show <scope> --property=MemoryMax`.
    /// Returns `None` if the scope is gone, the query fails, or the
    /// property is `infinity` (no limit set).
    pub fn memory_max_mb(&self) -> Option<u64> {
        self.query_scope_properties()
            .and_then(|properties| properties.memory_max_bytes)
            .map(bytes_to_mb)
    }

    /// Produce a diagnostic hint string when OOM is detected.
    ///
    /// Consolidates all systemd queries into a single `systemctl show`
    /// call to minimize subprocess overhead.  Returns `Some(hint)` with
    /// peak/limit info and config advice if OOM was triggered, `None`
    /// otherwise. When the failed scope has already been GC'd, use
    /// [`Self::oom_diagnosis_with_signal`] to fall back to SIGKILL inference.
    pub fn oom_diagnosis(&self) -> Option<String> {
        self.oom_diagnosis_with_signal(None)
    }

    /// Produce an actionable OOM diagnosis, falling back to SIGKILL when the
    /// failed scope has already been GC'd by systemd.
    pub fn oom_diagnosis_with_signal(&self, exit_signal: Option<i32>) -> Option<String> {
        match self.query_scope_properties() {
            Some(properties) if properties.result.as_deref() == Some("oom-kill") => {
                Some(self.format_oom_diagnosis(
                    properties.memory_peak_bytes,
                    properties.memory_max_bytes,
                    properties.memory_swap_max_bytes,
                    false,
                ))
            }
            Some(properties) if properties.is_empty() && self.should_assume_oom(exit_signal) => {
                Some(self.format_oom_diagnosis(None, None, None, true))
            }
            Some(_) => None,
            None if self.should_assume_oom(exit_signal) => {
                Some(self.format_oom_diagnosis(None, None, None, true))
            }
            None => None,
        }
    }

    fn query_scope_properties(&self) -> Option<ScopeProperties> {
        let output = Command::new("systemctl")
            .args([
                "--user",
                "show",
                &self.scope_name,
                "--property=LoadState,Result,MemoryPeak,MemoryMax,MemorySwapMax",
            ])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut properties = ScopeProperties::default();
        for line in stdout.lines() {
            if let Some(value) = line.strip_prefix("LoadState=") {
                let value = value.trim();
                if !value.is_empty() {
                    properties.load_state = Some(value.to_string());
                }
            } else if let Some(value) = line.strip_prefix("Result=") {
                let value = value.trim();
                if !value.is_empty() {
                    properties.result = Some(value.to_string());
                }
            } else if let Some(value) = line.strip_prefix("MemoryPeak=") {
                properties.memory_peak_bytes = parse_memory_property(value);
            } else if let Some(value) = line.strip_prefix("MemoryMax=") {
                properties.memory_max_bytes = parse_memory_property(value);
            } else if let Some(value) = line.strip_prefix("MemorySwapMax=") {
                properties.memory_swap_max_bytes = parse_memory_property(value);
            }
        }
        if properties.is_not_found() {
            return None;
        }
        Some(properties)
    }

    fn should_assume_oom(&self, exit_signal: Option<i32>) -> bool {
        exit_signal == Some(SIGKILL) && self.configured_memory_max_mb > 0
    }

    fn format_oom_diagnosis(
        &self,
        peak_bytes: Option<u64>,
        memory_max_bytes: Option<u64>,
        memory_swap_max_bytes: Option<u64>,
        inferred_from_sigkill: bool,
    ) -> String {
        let peak = peak_bytes
            .map(|bytes| format!("peak: {}MB", bytes_to_mb(bytes)))
            .unwrap_or_else(|| "peak: unknown".to_string());
        let limit_mb = memory_max_bytes
            .map(bytes_to_mb)
            .unwrap_or(self.configured_memory_max_mb);
        let swap_mb = memory_swap_max_bytes
            .map(bytes_to_mb)
            .or(self.configured_memory_swap_max_mb);
        let mut message = format!(
            "process was {} ({peak}, limit: {limit_mb}MB, {}). \
             Increase resources.memory_max_mb or tools.<tool>.memory_max_mb \
             in .csa/config.toml",
            if inferred_from_sigkill {
                "likely OOM-killed after scope cleanup"
            } else {
                "OOM-killed"
            },
            format_swap_limit(swap_mb),
        );
        if swap_mb == Some(0) {
            message.push_str(
                " Swap is disabled; consider increasing resources.memory_swap_max_mb \
                 or tools.<tool>.memory_swap_max_mb.",
            );
        }
        message
    }

    /// Explicitly stop the scope, using the same cleanup path as [`Drop`].
    pub fn stop(self) {
        drop(self);
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

    fn reset_failed_scope(&self) {
        if !self.collect_mode_persists_failures {
            return;
        }

        debug!(scope = %self.scope_name, "resetting failed cgroup scope");
        let result = Command::new("systemctl")
            .args(["--user", "reset-failed", &self.scope_name])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        match result {
            Ok(status) if status.success() => {
                debug!(scope = %self.scope_name, "scope reset-failed completed");
            }
            Ok(status) => {
                debug!(
                    scope = %self.scope_name,
                    code = status.code(),
                    "scope reset-failed returned non-zero (may already be gone)"
                );
            }
            Err(e) => {
                warn!(
                    scope = %self.scope_name,
                    error = %e,
                    "failed to run systemctl reset-failed"
                );
            }
        }
    }
}

impl Drop for CgroupScopeGuard {
    fn drop(&mut self) {
        self.stop_scope();
        self.reset_failed_scope();
    }
}

fn parse_memory_property(value: &str) -> Option<u64> {
    let trimmed = value.trim();
    if trimmed == "infinity" || trimmed.is_empty() {
        return None;
    }
    trimmed.parse::<u64>().ok()
}

fn bytes_to_mb(bytes: u64) -> u64 {
    bytes / 1024 / 1024
}

fn format_swap_limit(swap_mb: Option<u64>) -> String {
    match swap_mb {
        Some(swap) => format!("swap: {swap}MB"),
        None => "swap: system default".to_string(),
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
#[path = "cgroup_tests.rs"]
mod tests;
