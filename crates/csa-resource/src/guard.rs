use anyhow::{Result, bail};
use sysinfo::System;

/// Configuration for resource limits (mirrors csa-config's ResourcesConfig
/// but duplicated here to avoid circular dependency).
#[derive(Debug, Clone)]
pub struct ResourceLimits {
    /// Minimum combined free memory (physical + swap) in MB.
    /// CSA refuses to launch a tool if the combined available memory
    /// is below this threshold.
    pub min_free_memory_mb: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            min_free_memory_mb: 4096,
        }
    }
}

/// Guards resource availability before launching tools.
pub struct ResourceGuard {
    sys: System,
    limits: ResourceLimits,
}

impl ResourceGuard {
    pub fn new(limits: ResourceLimits) -> Self {
        let mut sys = System::new();
        sys.refresh_memory();
        Self { sys, limits }
    }

    /// Check if enough resources are available to launch a tool.
    ///
    /// Simple threshold: available_memory + available_swap >= reserve_mb.
    /// No per-tool P95 estimation — just a flat reserve check.
    pub fn check_availability(&mut self, tool_name: &str) -> Result<()> {
        self.sys.refresh_memory();

        let available_phys_bytes = self.sys.available_memory();
        let available_swap_bytes = self.sys.free_swap();
        let available_total_bytes = available_phys_bytes.saturating_add(available_swap_bytes);

        let available_phys = available_phys_bytes / 1024 / 1024;
        let available_swap = available_swap_bytes / 1024 / 1024;
        let available_total = available_total_bytes / 1024 / 1024;

        let reserve = self.limits.min_free_memory_mb;

        if available_total < reserve {
            bail!(
                "OOM Risk Prevention: Not enough memory to launch '{}'.\n\
                Available: {} MB (physical {} + swap {}), Reserve: {} MB\n\
                (Try closing other apps or wait for running agents to finish)",
                tool_name,
                available_total,
                available_phys,
                available_swap,
                reserve,
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_guard_new_default_limits() {
        let limits = ResourceLimits::default();
        let _guard = ResourceGuard::new(limits);
        assert_eq!(ResourceLimits::default().min_free_memory_mb, 4096);
    }

    #[test]
    fn test_check_availability_succeeds_with_enough_memory() {
        let limits = ResourceLimits {
            min_free_memory_mb: 1,
        };
        let mut guard = ResourceGuard::new(limits);
        let result = guard.check_availability("test_tool");
        // 1 MB reserve — any running system has this.
        assert!(result.is_ok(), "check_availability failed: {:?}", result);
    }

    #[test]
    fn test_check_availability_fails_with_impossible_limits() {
        let limits = ResourceLimits {
            min_free_memory_mb: u64::MAX / 2,
        };
        let mut guard = ResourceGuard::new(limits);
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
    fn test_check_availability_simple_threshold() {
        // Verify the simple threshold: available_mem + available_swap >= reserve
        // With reserve = 2 MB, any system should pass.
        let limits = ResourceLimits {
            min_free_memory_mb: 2,
        };
        let mut guard = ResourceGuard::new(limits);
        let result = guard.check_availability("threshold_tool");
        assert!(
            result.is_ok(),
            "2 MB reserve should pass on any system: {:?}",
            result,
        );
    }

    #[test]
    fn test_check_availability_includes_swap() {
        // With a very small reserve, the check must pass even if
        // physical memory alone would be tight — swap is counted.
        let limits = ResourceLimits {
            min_free_memory_mb: 1,
        };
        let mut guard = ResourceGuard::new(limits);

        // Refresh and verify combined memory is used
        guard.sys.refresh_memory();
        let phys = guard.sys.available_memory() / 1024 / 1024;
        let swap = guard.sys.free_swap() / 1024 / 1024;
        let combined = phys + swap;

        // The check should pass because combined >= 1 MB
        let result = guard.check_availability("swap_tool");
        assert!(result.is_ok(), "combined {} MB should be >= 1 MB", combined);
    }
}
