use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::config::EnforcementMode;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourcesConfig {
    /// Minimum physical MemAvailable in MB before refusing launch.
    #[serde(default = "default_min_mem")]
    pub min_free_memory_mb: u64,
    /// Kill child if no streamed output for this many consecutive seconds.
    #[serde(default = "default_idle_timeout_seconds")]
    pub idle_timeout_seconds: u64,
    /// After entering idle liveness mode, terminate only after this many
    /// consecutive seconds with no positive liveness signal.
    #[serde(default = "default_liveness_dead_seconds")]
    pub liveness_dead_seconds: Option<u64>,
    /// Fatal backend/provider error markers that should trigger a fast-fail
    /// watchdog path after output has stopped making progress.
    #[serde(default = "default_fatal_error_markers")]
    pub fatal_error_markers: Vec<String>,
    /// Whether the fatal-error-marker silent-hang scan (#1652) is enabled.
    ///
    /// When `true` (the default), a session is fast-failed if a configured
    /// provider/4xx/5xx marker appears in its relayed output and no further
    /// output progress is observed for the no-progress grace window. Set to
    /// `false` to opt out for sessions that legitimately read/edit/test CSA's
    /// own error/quota/failover detection code, whose source and test fixtures
    /// contain marker literals (e.g. "429", "quota exceeded") that are the
    /// SUBJECT of the work rather than an actual backend error (#1745).
    ///
    /// Disabling this knob bypasses ONLY the marker-based fatal classification;
    /// the idle-timeout and wall-clock timeout still apply.
    #[serde(default = "default_error_marker_scan")]
    pub error_marker_scan: bool,
    /// Whether post-run hook-bypass command/env scanning is enabled.
    ///
    /// When `true` (the default), CSA rejects sessions whose executed command
    /// events or explicit child process env show hook-bypass forms such as
    /// `git commit --no-verify`, `git push --no-verify`, or `LEFTHOOK=0`.
    /// Set to `false` only for emergency recovery; the scan is deliberately
    /// scoped to executed commands/env rather than prompt or output text.
    #[serde(default = "default_hook_bypass_scan")]
    pub hook_bypass_scan: bool,
    /// Maximum time to block when waiting for a free global tool slot.
    #[serde(default = "default_slot_wait_timeout_seconds")]
    pub slot_wait_timeout_seconds: u64,
    /// Maximum time to write prompt payload to child stdin.
    #[serde(default = "default_stdin_write_timeout_seconds")]
    pub stdin_write_timeout_seconds: u64,
    /// Grace period between SIGTERM and SIGKILL during forced termination.
    #[serde(default = "default_termination_grace_period_seconds")]
    pub termination_grace_period_seconds: u64,
    /// Maximum time to wait for the first response from a backend tool.
    /// When set, uses a shorter timeout until the first output is received,
    /// then falls back to `idle_timeout_seconds`.  This detects "backend never
    /// started" much faster than waiting the full idle timeout.
    #[serde(default)]
    pub initial_response_timeout_seconds: Option<u64>,
    /// Deprecated: initial memory estimates per tool.
    /// Retained for backward-compatible deserialization of old configs.
    /// New configs should NOT include this field.
    #[serde(default, skip_serializing)]
    pub initial_estimates: HashMap<String, u64>,
    /// Sandbox enforcement mode for resource limits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enforcement_mode: Option<EnforcementMode>,
    /// Maximum physical memory (RSS) in MB for child tool processes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_max_mb: Option<u64>,
    /// Maximum swap usage in MB for child tool processes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_swap_max_mb: Option<u64>,
    /// Default Node.js heap size limit (MB) for child tool processes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_heap_limit_mb: Option<u64>,
    /// Maximum number of PIDs for child tool process trees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pids_max: Option<u32>,
    /// Soft memory limit as a percentage of `memory_max_mb`.
    /// When current memory usage exceeds this threshold, the monitor sends
    /// SIGTERM to the process group.  Default: 70 (%).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soft_limit_percent: Option<u8>,
    /// Polling interval for the memory monitor in seconds.  Default: 5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_monitor_interval_seconds: Option<u64>,
}

fn default_min_mem() -> u64 {
    4096
}

fn default_idle_timeout_seconds() -> u64 {
    250
}

fn default_slot_wait_timeout_seconds() -> u64 {
    250
}

fn default_liveness_dead_seconds() -> Option<u64> {
    Some(600)
}

fn default_fatal_error_markers() -> Vec<String> {
    [
        "HTTP 400",
        "HTTP 401",
        "HTTP 403",
        "HTTP 404",
        "HTTP 408",
        "HTTP 409",
        "HTTP 429",
        "HTTP 500",
        "HTTP 502",
        "HTTP 503",
        "HTTP 504",
        "status 400",
        "status 401",
        "status 403",
        "status 404",
        "status 408",
        "status 409",
        "status 429",
        "status 500",
        "status 502",
        "status 503",
        "status 504",
        "400 Bad Request",
        "401 Unauthorized",
        "403 Forbidden",
        "404 Not Found",
        "408 Request Timeout",
        "409 Conflict",
        "429 Too Many Requests",
        "500 Internal Server Error",
        "502 Bad Gateway",
        "503 Service Unavailable",
        "504 Gateway Timeout",
        // Provider/quota/auth envelope tokens — high-specificity strings that do not occur in
        // benign agent prose. Bare phrases like "rate limit" / "provider error" / "overloaded"
        // were removed (#1652): they matched normal model output (e.g. "avoid the rate limit")
        // which, combined with a >30s no-progress pause, fast-failed healthy live sessions.
        // Keep in sync with tool_liveness_fatal_error.rs::default_tier1_fatal_error_markers.
        "rate_limit_exceeded",
        "rate limit exceeded",
        "insufficient_quota",
        "insufficient quota",
        "quota exceeded",
        "QUOTA_EXHAUSTED",
        "TerminalQuotaError",
        "overloaded_error",
        "invalid_api_key",
        "API key not found",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// Serde default for [`ResourcesConfig::error_marker_scan`].
///
/// An ABSENT field MUST deserialize to `true` so the #1652 scan stays enabled
/// by default; `#[serde(default)]` would yield `false` (scan disabled), which
/// would silently weaken existing behavior (rust rule 016 / serde-default).
fn default_error_marker_scan() -> bool {
    true
}

/// Serde default for [`ResourcesConfig::hook_bypass_scan`].
///
/// An ABSENT field MUST deserialize to `true`; `#[serde(default)]` would yield
/// `false` and silently disable hook-bypass enforcement.
fn default_hook_bypass_scan() -> bool {
    true
}

fn default_stdin_write_timeout_seconds() -> u64 {
    30
}

fn default_termination_grace_period_seconds() -> u64 {
    5
}

impl Default for ResourcesConfig {
    fn default() -> Self {
        Self {
            min_free_memory_mb: default_min_mem(),
            idle_timeout_seconds: default_idle_timeout_seconds(),
            liveness_dead_seconds: default_liveness_dead_seconds(),
            fatal_error_markers: default_fatal_error_markers(),
            error_marker_scan: default_error_marker_scan(),
            hook_bypass_scan: default_hook_bypass_scan(),
            slot_wait_timeout_seconds: default_slot_wait_timeout_seconds(),
            stdin_write_timeout_seconds: default_stdin_write_timeout_seconds(),
            termination_grace_period_seconds: default_termination_grace_period_seconds(),
            initial_response_timeout_seconds: None,
            initial_estimates: HashMap::new(),
            enforcement_mode: None,
            memory_max_mb: None,
            memory_swap_max_mb: None,
            node_heap_limit_mb: None,
            pids_max: None,
            soft_limit_percent: None,
            memory_monitor_interval_seconds: None,
        }
    }
}

impl ResourcesConfig {
    /// Returns true when all fields match their defaults.
    /// Used by `skip_serializing_if` to omit the `[resources]` section
    /// from minimal project configs.
    pub fn is_default(&self) -> bool {
        self.min_free_memory_mb == default_min_mem()
            && self.idle_timeout_seconds == default_idle_timeout_seconds()
            && self.liveness_dead_seconds == default_liveness_dead_seconds()
            && self.fatal_error_markers == default_fatal_error_markers()
            && self.error_marker_scan == default_error_marker_scan()
            && self.hook_bypass_scan == default_hook_bypass_scan()
            && self.slot_wait_timeout_seconds == default_slot_wait_timeout_seconds()
            && self.stdin_write_timeout_seconds == default_stdin_write_timeout_seconds()
            && self.termination_grace_period_seconds == default_termination_grace_period_seconds()
            && self.initial_response_timeout_seconds.is_none()
            && self.enforcement_mode.is_none()
            && self.memory_max_mb.is_none()
            && self.memory_swap_max_mb.is_none()
            && self.node_heap_limit_mb.is_none()
            && self.pids_max.is_none()
            && self.soft_limit_percent.is_none()
            && self.memory_monitor_interval_seconds.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #1745: an ABSENT `error_marker_scan` field MUST deserialize to `true`
    /// so the #1652 silent-hang scan stays enabled by default. This guards
    /// against a regression to `#[serde(default)]` (which would yield `false`).
    #[test]
    fn error_marker_scan_absent_deserializes_to_true() {
        let cfg: ResourcesConfig = toml::from_str("").expect("empty [resources] table");
        assert!(
            cfg.error_marker_scan,
            "absent error_marker_scan must default to true (scan enabled)"
        );
    }

    #[test]
    fn error_marker_scan_explicit_false_is_honored() {
        let cfg: ResourcesConfig =
            toml::from_str("error_marker_scan = false").expect("explicit false");
        assert!(!cfg.error_marker_scan);
    }

    #[test]
    fn error_marker_scan_explicit_true_is_honored() {
        let cfg: ResourcesConfig =
            toml::from_str("error_marker_scan = true").expect("explicit true");
        assert!(cfg.error_marker_scan);
    }

    #[test]
    fn hook_bypass_scan_absent_deserializes_to_true() {
        let cfg: ResourcesConfig = toml::from_str("").expect("empty [resources] table");
        assert!(
            cfg.hook_bypass_scan,
            "absent hook_bypass_scan must default to true (scan enabled)"
        );
    }

    #[test]
    fn hook_bypass_scan_explicit_false_is_honored() {
        let cfg: ResourcesConfig =
            toml::from_str("hook_bypass_scan = false").expect("explicit false");
        assert!(!cfg.hook_bypass_scan);
    }

    #[test]
    fn hook_bypass_scan_explicit_true_is_honored() {
        let cfg: ResourcesConfig =
            toml::from_str("hook_bypass_scan = true").expect("explicit true");
        assert!(cfg.hook_bypass_scan);
    }

    /// The struct `Default` must agree with the serde default so a freshly
    /// constructed config and a deserialized-empty config behave identically.
    #[test]
    fn default_impl_enables_error_marker_scan() {
        assert!(ResourcesConfig::default().error_marker_scan);
        assert!(ResourcesConfig::default().hook_bypass_scan);
        assert!(default_error_marker_scan());
        assert!(default_hook_bypass_scan());
    }

    /// `is_default()` must treat scan-enabled as the default state (so a config
    /// that only disables the scan is NOT omitted by `skip_serializing_if`).
    #[test]
    fn is_default_tracks_error_marker_scan() {
        assert!(ResourcesConfig::default().is_default());
        let disabled = ResourcesConfig {
            error_marker_scan: false,
            ..ResourcesConfig::default()
        };
        assert!(!disabled.is_default());
        let disabled = ResourcesConfig {
            hook_bypass_scan: false,
            ..ResourcesConfig::default()
        };
        assert!(!disabled.is_default());
    }
}
