use anyhow::{Result, bail};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use sysinfo::System;

use crate::stats::UsageStats;

/// Configuration for resource limits (mirrors csa-config's ResourcesConfig
/// but duplicated here to avoid circular dependency).
#[derive(Debug, Clone)]
pub struct ResourceLimits {
    /// Minimum free memory (physical + swap combined) in MB.
    /// CSA refuses to launch a tool if the combined free memory
    /// would drop below this threshold after accounting for the
    /// tool's estimated usage.
    pub min_free_memory_mb: u64,
    pub initial_estimates: HashMap<String, u64>,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            min_free_memory_mb: 4096,
            initial_estimates: HashMap::new(),
        }
    }
}

/// Guards resource availability before launching tools.
pub struct ResourceGuard {
    sys: System,
    limits: ResourceLimits,
    stats: UsageStats,
    stats_path: PathBuf,
}

impl ResourceGuard {
    pub fn new(limits: ResourceLimits, stats_path: &Path) -> Self {
        let mut sys = System::new();
        sys.refresh_memory();
        let stats = UsageStats::load(stats_path).unwrap_or_default();
        Self {
            sys,
            limits,
            stats,
            stats_path: stats_path.to_path_buf(),
        }
    }

    /// Check if enough resources are available to launch the given tool.
    ///
    /// Available memory = physical available + swap free (combined).
    /// Required = min_free_memory_mb (safety buffer) + estimated tool usage.
    pub fn check_availability(&mut self, tool_name: &str) -> Result<()> {
        self.sys.refresh_memory();

        // Add in bytes first, then convert to MB to avoid truncation error
        let available_phys_bytes = self.sys.available_memory();
        let available_swap_bytes = self.sys.free_swap();
        let available_total_bytes = available_phys_bytes.saturating_add(available_swap_bytes);

        let available_phys = available_phys_bytes / 1024 / 1024;
        let available_swap = available_swap_bytes / 1024 / 1024;
        let available_total = available_total_bytes / 1024 / 1024;

        // Prefer P95 historical estimate, fallback to initial config
        let estimated_usage = self
            .stats
            .get_p95_estimate(tool_name)
            .unwrap_or_else(|| *self.limits.initial_estimates.get(tool_name).unwrap_or(&500));

        let required = self
            .limits
            .min_free_memory_mb
            .saturating_add(estimated_usage);

        if available_total < required {
            bail!(
                "OOM Risk Prevention: Not enough memory to launch '{}'.\n\
                Available: {} MB (physical {} + swap {}), Min Buffer: {} MB, Est. Tool Usage: {} MB (P95)\n\
                (Try closing other apps or wait for running agents to finish)",
                tool_name,
                available_total,
                available_phys,
                available_swap,
                self.limits.min_free_memory_mb,
                estimated_usage
            );
        }

        Ok(())
    }

    /// Record tool's peak memory consumption.
    pub fn record_usage(&mut self, tool_name: &str, peak_memory_mb: u64) {
        self.stats.record(tool_name, peak_memory_mb);
        let _ = self.stats.save(&self.stats_path);
    }

    /// Get the current usage stats (for inspection/testing).
    pub fn stats(&self) -> &UsageStats {
        &self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_resource_guard_new_default_limits() {
        let dir = tempdir().unwrap();
        let stats_path = dir.path().join("stats.toml");
        let limits = ResourceLimits::default();
        let _guard = ResourceGuard::new(limits, &stats_path);
        // Should not panic
    }

    #[test]
    fn test_check_availability_succeeds_with_enough_memory() {
        let dir = tempdir().unwrap();
        let stats_path = dir.path().join("stats.toml");

        // Use minimal limits so the test passes on any system (including
        // macOS CI runners with limited memory and zero-reported swap).
        let mut initial_estimates = HashMap::new();
        initial_estimates.insert("test_tool".to_string(), 1);
        let limits = ResourceLimits {
            min_free_memory_mb: 1,
            initial_estimates,
        };

        let mut guard = ResourceGuard::new(limits, &stats_path);
        let result = guard.check_availability("test_tool");
        // required = 1 + 1 = 2 MB (physical + swap combined) — any running system has this.
        assert!(result.is_ok(), "check_availability failed: {:?}", result);
    }

    #[test]
    fn test_record_usage_updates_stats() {
        let dir = tempdir().unwrap();
        let stats_path = dir.path().join("stats.toml");
        let limits = ResourceLimits::default();

        let mut guard = ResourceGuard::new(limits, &stats_path);
        guard.record_usage("tool1", 500);

        // Verify stats were updated
        assert_eq!(guard.stats().get_p95_estimate("tool1"), Some(500));

        // Verify it was persisted
        let loaded_stats = UsageStats::load(&stats_path).unwrap();
        assert_eq!(loaded_stats.get_p95_estimate("tool1"), Some(500));
    }

    #[test]
    fn test_check_availability_fails_with_impossible_limits() {
        let dir = tempdir().unwrap();
        let stats_path = dir.path().join("stats.toml");

        // Require more memory than any machine has
        let limits = ResourceLimits {
            min_free_memory_mb: u64::MAX / 2,
            initial_estimates: HashMap::new(),
        };

        let mut guard = ResourceGuard::new(limits, &stats_path);
        let result = guard.check_availability("any_tool");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("OOM Risk Prevention"),
            "Expected OOM error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_check_availability_uses_p95_from_history() {
        let dir = tempdir().unwrap();
        let stats_path = dir.path().join("stats.toml");

        // Pre-populate stats with a massive P95 estimate
        let mut stats = UsageStats::default();
        stats.record("heavy_tool", u64::MAX / 4);
        stats.save(&stats_path).unwrap();

        // Even with generous min_free, the P95 estimate makes it fail
        let limits = ResourceLimits {
            min_free_memory_mb: 1,
            initial_estimates: HashMap::new(),
        };

        let mut guard = ResourceGuard::new(limits, &stats_path);
        let result = guard.check_availability("heavy_tool");
        assert!(result.is_err(), "Should fail due to enormous P95 estimate");
    }

    #[test]
    fn test_check_availability_uses_initial_estimate_fallback() {
        let dir = tempdir().unwrap();
        let stats_path = dir.path().join("stats.toml");

        // No history → falls back to initial_estimates
        let mut initial_estimates = HashMap::new();
        initial_estimates.insert("new_tool".to_string(), u64::MAX / 4);

        let limits = ResourceLimits {
            min_free_memory_mb: 1,
            initial_estimates,
        };

        let mut guard = ResourceGuard::new(limits, &stats_path);
        let result = guard.check_availability("new_tool");
        assert!(
            result.is_err(),
            "Should fail due to enormous initial estimate"
        );
    }

    #[test]
    fn test_check_availability_default_estimate_500mb() {
        let dir = tempdir().unwrap();
        let stats_path = dir.path().join("stats.toml");

        // No history, no initial_estimates → default 500 MB estimate.
        // With min_free=1, required = 1 + 500 = 501 MB — any normal
        // system has this much combined physical + swap memory.
        let limits = ResourceLimits {
            min_free_memory_mb: 1,
            initial_estimates: HashMap::new(),
        };

        let mut guard = ResourceGuard::new(limits, &stats_path);
        let result = guard.check_availability("unknown_tool");
        assert!(result.is_ok(), "501 MB should be available: {:?}", result);
    }

    #[test]
    fn test_record_usage_multiple_tools() {
        let dir = tempdir().unwrap();
        let stats_path = dir.path().join("stats.toml");
        let limits = ResourceLimits::default();

        let mut guard = ResourceGuard::new(limits, &stats_path);
        guard.record_usage("tool_a", 100);
        guard.record_usage("tool_b", 200);

        assert_eq!(guard.stats().get_p95_estimate("tool_a"), Some(100));
        assert_eq!(guard.stats().get_p95_estimate("tool_b"), Some(200));

        // Verify persistence of both tools
        let loaded = UsageStats::load(&stats_path).unwrap();
        assert_eq!(loaded.get_p95_estimate("tool_a"), Some(100));
        assert_eq!(loaded.get_p95_estimate("tool_b"), Some(200));
    }

    #[test]
    fn test_guard_loads_existing_stats() {
        let dir = tempdir().unwrap();
        let stats_path = dir.path().join("stats.toml");

        // Pre-populate stats file
        let mut stats = UsageStats::default();
        stats.record("pre_tool", 42);
        stats.save(&stats_path).unwrap();

        // New guard should load existing stats
        let limits = ResourceLimits::default();
        let guard = ResourceGuard::new(limits, &stats_path);
        assert_eq!(guard.stats().get_p95_estimate("pre_tool"), Some(42));
    }
}
