use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::config::EnforcementMode;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourcesConfig {
    /// Minimum combined free memory (physical + swap) in MB before refusing launch.
    #[serde(default = "default_min_mem")]
    pub min_free_memory_mb: u64,
    /// Kill child if no streamed output for this many consecutive seconds.
    #[serde(default = "default_idle_timeout_seconds")]
    pub idle_timeout_seconds: u64,
    /// After entering idle liveness mode, terminate only after this many
    /// consecutive seconds with no positive liveness signal.
    #[serde(default = "default_liveness_dead_seconds")]
    pub liveness_dead_seconds: Option<u64>,
    /// Maximum time to block when waiting for a free global tool slot.
    #[serde(default = "default_slot_wait_timeout_seconds")]
    pub slot_wait_timeout_seconds: u64,
    /// Maximum time to write prompt payload to child stdin.
    #[serde(default = "default_stdin_write_timeout_seconds")]
    pub stdin_write_timeout_seconds: u64,
    /// Grace period between SIGTERM and SIGKILL during forced termination.
    #[serde(default = "default_termination_grace_period_seconds")]
    pub termination_grace_period_seconds: u64,
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
}

fn default_min_mem() -> u64 {
    4096
}

fn default_idle_timeout_seconds() -> u64 {
    300
}

fn default_slot_wait_timeout_seconds() -> u64 {
    300
}

fn default_liveness_dead_seconds() -> Option<u64> {
    Some(600)
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
            slot_wait_timeout_seconds: default_slot_wait_timeout_seconds(),
            stdin_write_timeout_seconds: default_stdin_write_timeout_seconds(),
            termination_grace_period_seconds: default_termination_grace_period_seconds(),
            initial_estimates: HashMap::new(),
            enforcement_mode: None,
            memory_max_mb: None,
            memory_swap_max_mb: None,
            node_heap_limit_mb: None,
            pids_max: None,
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
            && self.slot_wait_timeout_seconds == default_slot_wait_timeout_seconds()
            && self.stdin_write_timeout_seconds == default_stdin_write_timeout_seconds()
            && self.termination_grace_period_seconds == default_termination_grace_period_seconds()
            && self.enforcement_mode.is_none()
            && self.memory_max_mb.is_none()
            && self.memory_swap_max_mb.is_none()
            && self.node_heap_limit_mb.is_none()
            && self.pids_max.is_none()
    }
}
