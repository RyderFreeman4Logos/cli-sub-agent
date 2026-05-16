use anyhow::{Result, bail};
use sysinfo::System;
use tracing::warn;

/// Configuration for resource limits (mirrors csa-config's ResourcesConfig
/// but duplicated here to avoid circular dependency).
#[derive(Debug, Clone)]
pub struct ResourceLimits {
    /// Minimum physical MemAvailable in MB.
    /// CSA refuses to launch a tool if physical available memory is below
    /// this threshold. Swap is reported for diagnostics but is not counted
    /// toward the hard pre-spawn gate.
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
    /// Two-tier threshold:
    /// - **Hard block**: MemAvailable < reserve_mb → refuse to launch.
    /// - **Warning**: MemAvailable < 150% of reserve_mb → warn but allow.
    pub fn check_availability(&mut self, tool_name: &str) -> Result<()> {
        self.sys.refresh_memory();

        let available_phys_bytes = self.sys.available_memory();
        let available_swap_bytes = self.sys.free_swap();
        let available_phys = available_phys_bytes / 1024 / 1024;
        let available_swap = available_swap_bytes / 1024 / 1024;
        let available_combined = available_phys.saturating_add(available_swap);

        evaluate_memory_availability(
            tool_name,
            available_phys,
            available_swap,
            available_combined,
            self.limits.min_free_memory_mb,
        )
    }

    /// Warn if configured cgroup limits exceed a percentage of total system RAM.
    ///
    /// Emits a `tracing::warn!` if `memory_max_mb + memory_swap_max_mb` exceeds
    /// `warn_threshold_percent` of total physical RAM.  This is an advisory
    /// check — it does **not** block execution.
    pub fn check_health(
        &mut self,
        memory_max_mb: Option<u64>,
        memory_swap_max_mb: Option<u64>,
        warn_threshold_percent: u8,
    ) {
        let configured_mb = memory_max_mb.unwrap_or(0) + memory_swap_max_mb.unwrap_or(0);
        if configured_mb == 0 {
            return;
        }

        self.sys.refresh_memory();
        let total_ram_mb = self.sys.total_memory() / 1024 / 1024;
        if total_ram_mb == 0 {
            return;
        }

        let threshold_mb = total_ram_mb * u64::from(warn_threshold_percent) / 100;

        if configured_mb > threshold_mb {
            warn!(
                configured_mb,
                total_ram_mb,
                threshold_percent = warn_threshold_percent,
                "cgroup memory limits ({configured_mb} MB) exceed \
                 {warn_threshold_percent}% of system RAM ({total_ram_mb} MB). \
                 This may cause excessive swapping or OOM kills. \
                 Reduce resources.memory_max_mb in .csa/config.toml"
            );
        }
    }
}

/// Multiplier for the warning threshold (150% of reserve).
const WARNING_MULTIPLIER_NUM: u64 = 3;
const WARNING_MULTIPLIER_DEN: u64 = 2;

/// Pure evaluation of memory availability against reserve.
///
/// - Hard block when `available_phys_mb < reserve_mb`.
/// - Warning when `available_phys_mb < reserve_mb * 150%`.
/// - Silent pass otherwise.
fn evaluate_memory_availability(
    tool_name: &str,
    available_phys_mb: u64,
    available_swap_mb: u64,
    available_combined_mb: u64,
    reserve_mb: u64,
) -> Result<()> {
    if available_phys_mb < reserve_mb {
        let message = format!(
            "CSA: low memory — available={available_phys_mb}MB < required={reserve_mb}MB. \
             Refusing to spawn tool scopes. actual_available_mb={available_phys_mb} \
             required_mb={reserve_mb} physical_available_mb={available_phys_mb} \
             swap_available_mb={available_swap_mb} combined_available_mb={available_combined_mb}"
        );
        eprintln!("{message}");
        bail!(
            "{message}. Free system memory or reduce [resources] min_free_memory_mb in \
             .csa/config.toml."
        );
    }

    let warn_threshold = reserve_mb.saturating_mul(WARNING_MULTIPLIER_NUM) / WARNING_MULTIPLIER_DEN;
    if available_phys_mb < warn_threshold {
        eprintln!(
            "CSA: low memory warning — available={available_phys_mb}MB < \
             warning={warn_threshold}MB; required={reserve_mb}MB. Proceeding, but tool scopes \
             may hit memory pressure."
        );
        warn!(
            available_mb = available_phys_mb,
            reserve_mb,
            warn_threshold_mb = warn_threshold,
            "Low memory: {available_phys_mb}MB available for '{tool_name}' \
             (reserve {reserve_mb}MB, warning threshold {warn_threshold}MB). \
             Session will proceed but may hit OOM pressure."
        );
    }

    Ok(())
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
        assert!(result.is_ok(), "check_availability failed: {result:?}");
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
            err_msg.contains("CSA: low memory"),
            "Expected memory error, got: {err_msg}"
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
            "2 MB reserve should pass on any system: {result:?}",
        );
    }

    #[test]
    fn test_check_availability_reports_swap_without_requiring_it() {
        // With a very small reserve, the check must pass even if
        // swap is present. The hard gate is based on physical MemAvailable.
        let limits = ResourceLimits {
            min_free_memory_mb: 1,
        };
        let mut guard = ResourceGuard::new(limits);

        // Refresh and verify the host reports physical memory.
        guard.sys.refresh_memory();
        let phys = guard.sys.available_memory() / 1024 / 1024;
        let swap = guard.sys.free_swap() / 1024 / 1024;

        let result = guard.check_availability("swap_tool");
        assert!(
            result.is_ok(),
            "physical {phys} MB with swap {swap} MB should be >= 1 MB"
        );
    }

    // --- Pure function tests (deterministic, no sysinfo dependency) ---

    #[test]
    fn test_evaluate_hard_block_when_available_below_reserve() {
        let result = evaluate_memory_availability("test_tool", 3000, 1000, 4000, 4096);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("CSA: low memory"),
            "Expected hard block, got: {msg}"
        );
        assert!(
            msg.contains("actual_available_mb=3000"),
            "Should show available: {msg}"
        );
        assert!(
            msg.contains("required_mb=4096"),
            "Should show reserve: {msg}"
        );
    }

    #[test]
    fn test_evaluate_warning_when_available_between_100_and_150_percent() {
        // reserve=4096, 150% = 6144. available=5000 is between 4096..6144.
        let result = evaluate_memory_availability("test_tool", 5000, 1000, 6000, 4096);
        // Should succeed (warning only, no error).
        assert!(result.is_ok(), "Should warn but not block: {result:?}");
    }

    #[test]
    fn test_evaluate_blocks_when_memavailable_below_reserve_even_with_swap() {
        let result = evaluate_memory_availability("test_tool", 3900, 4096, 7996, 4096);
        assert!(
            result.is_err(),
            "swap must not satisfy min_free_memory_mb when MemAvailable is low"
        );
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("actual_available_mb=3900"));
        assert!(msg.contains("swap_available_mb=4096"));
        assert!(msg.contains("combined_available_mb=7996"));
    }

    #[test]
    fn test_evaluate_no_warning_when_available_above_150_percent() {
        // reserve=4096, 150% = 6144. available=7000 is above 6144.
        let result = evaluate_memory_availability("test_tool", 7000, 1000, 8000, 4096);
        assert!(result.is_ok(), "Should pass without warning: {result:?}");
    }

    #[test]
    fn test_evaluate_exact_boundary_at_reserve() {
        // Exactly at reserve — should pass (not strictly less than).
        let result = evaluate_memory_availability("test_tool", 4096, 1096, 5192, 4096);
        assert!(result.is_ok(), "Exact reserve should pass: {result:?}");
    }

    #[test]
    fn test_evaluate_exact_boundary_at_warning_threshold() {
        // reserve=4096, 150% = 6144. available=6144 is exactly at warning threshold.
        let result = evaluate_memory_availability("test_tool", 6144, 1144, 7288, 4096);
        // 6144 is NOT < 6144, so no warning.
        assert!(
            result.is_ok(),
            "Exact warning threshold should pass: {result:?}"
        );
    }
}
