use anyhow::Result;
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

/// Host-memory projection for a session spawn admission decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnMemoryAdmission {
    /// Memory the new session is expected to be allowed to consume.
    pub projected_spawn_mb: u64,
    /// Aggregate sampled RSS from already-active CSA session trees.
    pub active_session_rss_mb: u64,
    /// Aggregate active-session pressure after per-session projections are applied.
    pub active_session_projected_mb: u64,
    /// Number of active CSA sessions considered, excluding the session being spawned.
    pub active_session_count: u64,
    /// Number of active sessions whose process tree RSS was sampled successfully.
    pub sampled_session_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryAdmissionKind {
    Reserve,
    HostSpawn,
    ActiveSession,
}

impl MemoryAdmissionKind {
    pub const fn denial_class(self) -> &'static str {
        match self {
            Self::Reserve | Self::HostSpawn | Self::ActiveSession => "host_memory_admission",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryAdmissionError {
    message: String,
    pub kind: MemoryAdmissionKind,
    pub available_phys_mb: u64,
    pub available_swap_mb: u64,
    pub available_combined_mb: u64,
    pub total_ram_mb: u64,
    pub reserve_mb: u64,
    pub required_available_mb: Option<u64>,
    pub projected_spawn_mb: Option<u64>,
    pub active_session_rss_mb: Option<u64>,
    pub active_session_projected_mb: Option<u64>,
    pub active_session_count: Option<u64>,
    pub sampled_session_count: Option<u64>,
}

impl MemoryAdmissionError {
    fn new(message: String, kind: MemoryAdmissionKind, snapshot: MemoryAdmissionSnapshot) -> Self {
        Self {
            message,
            kind,
            available_phys_mb: snapshot.available_phys_mb,
            available_swap_mb: snapshot.available_swap_mb,
            available_combined_mb: snapshot.available_combined_mb,
            total_ram_mb: snapshot.total_ram_mb,
            reserve_mb: snapshot.reserve_mb,
            required_available_mb: snapshot.required_available_mb,
            projected_spawn_mb: snapshot.projected_spawn_mb,
            active_session_rss_mb: snapshot.active_session_rss_mb,
            active_session_projected_mb: snapshot.active_session_projected_mb,
            active_session_count: snapshot.active_session_count,
            sampled_session_count: snapshot.sampled_session_count,
        }
    }
}

impl std::fmt::Display for MemoryAdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for MemoryAdmissionError {}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct MemoryAdmissionSnapshot {
    available_phys_mb: u64,
    available_swap_mb: u64,
    available_combined_mb: u64,
    total_ram_mb: u64,
    reserve_mb: u64,
    required_available_mb: Option<u64>,
    projected_spawn_mb: Option<u64>,
    active_session_rss_mb: Option<u64>,
    active_session_projected_mb: Option<u64>,
    active_session_count: Option<u64>,
    sampled_session_count: Option<u64>,
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
        self.check_availability_with_admission(tool_name, None)
    }

    /// Check host memory with active-session and new-spawn projection.
    pub fn check_availability_with_admission(
        &mut self,
        tool_name: &str,
        admission: Option<SpawnMemoryAdmission>,
    ) -> Result<()> {
        self.sys.refresh_memory();

        let available_phys_bytes = self.sys.available_memory();
        let available_swap_bytes = self.sys.free_swap();
        let total_ram_bytes = self.sys.total_memory();
        let available_phys = available_phys_bytes / 1024 / 1024;
        let available_swap = available_swap_bytes / 1024 / 1024;
        let total_ram = total_ram_bytes / 1024 / 1024;
        let available_combined = available_phys.saturating_add(available_swap);

        evaluate_memory_availability(
            tool_name,
            available_phys,
            available_swap,
            available_combined,
            total_ram,
            self.limits.min_free_memory_mb,
            admission,
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
const ACTIVE_SESSION_SAFE_FRACTION_NUM: u64 = 3;
const ACTIVE_SESSION_SAFE_FRACTION_DEN: u64 = 4;
const ACTIVE_SESSION_WARNING_FRACTION_NUM: u64 = 9;
const ACTIVE_SESSION_WARNING_FRACTION_DEN: u64 = 10;

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
    total_ram_mb: u64,
    reserve_mb: u64,
    admission: Option<SpawnMemoryAdmission>,
) -> Result<()> {
    let base_snapshot = MemoryAdmissionSnapshot {
        available_phys_mb,
        available_swap_mb,
        available_combined_mb,
        total_ram_mb,
        reserve_mb,
        ..Default::default()
    };

    if available_phys_mb < reserve_mb {
        let message = format!(
            "CSA: low memory — available={available_phys_mb}MB < required={reserve_mb}MB. \
             Refusing to spawn tool scopes. actual_available_mb={available_phys_mb} \
             required_mb={reserve_mb} physical_available_mb={available_phys_mb} \
             swap_available_mb={available_swap_mb} combined_available_mb={available_combined_mb}"
        );
        eprintln!("{message}");
        let error = MemoryAdmissionError::new(
            format!(
                "{message}. Free system memory or reduce [resources] min_free_memory_mb in \
                 .csa/config.toml. For one csa run, pass --min-free-memory-mb <MB>."
            ),
            MemoryAdmissionKind::Reserve,
            base_snapshot,
        );
        return Err(error.into());
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

    if let Some(admission) = admission
        && admission.projected_spawn_mb > 0
    {
        let required_available_mb = reserve_mb.saturating_add(admission.projected_spawn_mb);
        if available_phys_mb < required_available_mb {
            let message = format!(
                "CSA: host memory admission denied — available={available_phys_mb}MB < \
                 required={required_available_mb}MB (reserve={reserve_mb}MB + \
                 projected_spawn={projected_spawn_mb}MB). active_sessions={active_sessions} \
                 sampled_sessions={sampled_sessions} active_session_rss_mb={active_rss} \
                 active_session_projected_mb={active_projected} swap_available_mb={available_swap_mb} \
                 combined_available_mb={available_combined_mb}",
                projected_spawn_mb = admission.projected_spawn_mb,
                active_sessions = admission.active_session_count,
                sampled_sessions = admission.sampled_session_count,
                active_rss = admission.active_session_rss_mb,
                active_projected = admission.active_session_projected_mb,
            );
            eprintln!("{message}");
            let error = MemoryAdmissionError::new(
                format!(
                    "{message}. Free host memory, wait for active CSA sessions to finish, or lower \
                     tool memory limits before spawning more work. Host admission uses physical \
                     MemAvailable only; swap and combined memory are reported for diagnostics but \
                     do not satisfy this pre-spawn gate. For one csa run, pass \
                     --memory-max-mb <MB> to lower projected_spawn or --min-free-memory-mb <MB> \
                     to lower the reserve. Persistent config keys: resources.memory_max_mb, \
                     tools.<tool>.memory_max_mb, resources.min_free_memory_mb."
                ),
                MemoryAdmissionKind::HostSpawn,
                MemoryAdmissionSnapshot {
                    required_available_mb: Some(required_available_mb),
                    projected_spawn_mb: Some(admission.projected_spawn_mb),
                    active_session_rss_mb: Some(admission.active_session_rss_mb),
                    active_session_projected_mb: Some(admission.active_session_projected_mb),
                    active_session_count: Some(admission.active_session_count),
                    sampled_session_count: Some(admission.sampled_session_count),
                    ..base_snapshot
                },
            );
            return Err(error.into());
        }

        let host_safe_limit_mb = total_ram_mb.saturating_mul(ACTIVE_SESSION_SAFE_FRACTION_NUM)
            / ACTIVE_SESSION_SAFE_FRACTION_DEN;
        let projected_active_mb = admission
            .active_session_projected_mb
            .saturating_add(admission.projected_spawn_mb);
        if host_safe_limit_mb > 0 && projected_active_mb > host_safe_limit_mb {
            let message = format!(
                "CSA: active-session memory admission denied — projected_active={projected_active_mb}MB \
                 > host_safe_limit={host_safe_limit_mb}MB ({safe_num}/{safe_den} of total_ram={total_ram_mb}MB). \
                 active_sessions={active_sessions} sampled_sessions={sampled_sessions} \
                 active_session_rss_mb={active_rss} active_session_projected_mb={active_projected} \
                 projected_spawn_mb={projected_spawn_mb} available_mb={available_phys_mb}",
                safe_num = ACTIVE_SESSION_SAFE_FRACTION_NUM,
                safe_den = ACTIVE_SESSION_SAFE_FRACTION_DEN,
                active_sessions = admission.active_session_count,
                sampled_sessions = admission.sampled_session_count,
                active_rss = admission.active_session_rss_mb,
                active_projected = admission.active_session_projected_mb,
                projected_spawn_mb = admission.projected_spawn_mb,
            );
            eprintln!("{message}");
            let error = MemoryAdmissionError::new(
                format!(
                    "{message}. CSA is refusing to launch work that could collectively exhaust host RAM. \
                     For one csa run, pass --memory-max-mb <MB> to lower projected_spawn. \
                     Persistent config keys: resources.memory_max_mb or tools.<tool>.memory_max_mb."
                ),
                MemoryAdmissionKind::ActiveSession,
                MemoryAdmissionSnapshot {
                    required_available_mb: Some(host_safe_limit_mb),
                    projected_spawn_mb: Some(admission.projected_spawn_mb),
                    active_session_rss_mb: Some(admission.active_session_rss_mb),
                    active_session_projected_mb: Some(admission.active_session_projected_mb),
                    active_session_count: Some(admission.active_session_count),
                    sampled_session_count: Some(admission.sampled_session_count),
                    ..base_snapshot
                },
            );
            return Err(error.into());
        }

        let host_warning_limit_mb = host_safe_limit_mb
            .saturating_mul(ACTIVE_SESSION_WARNING_FRACTION_NUM)
            / ACTIVE_SESSION_WARNING_FRACTION_DEN;
        if host_warning_limit_mb > 0 && projected_active_mb > host_warning_limit_mb {
            eprintln!(
                "CSA: active-session memory warning — projected_active={projected_active_mb}MB is \
                 near host_safe_limit={host_safe_limit_mb}MB; active_sessions={active_sessions}.",
                active_sessions = admission.active_session_count,
            );
            warn!(
                tool = tool_name,
                projected_active_mb,
                host_safe_limit_mb,
                active_sessions = admission.active_session_count,
                sampled_sessions = admission.sampled_session_count,
                "Active CSA session memory is near host admission limit"
            );
        }
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
        let result =
            evaluate_memory_availability("test_tool", 3000, 1000, 4000, 32_000, 4096, None);
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
        assert!(msg.contains("--min-free-memory-mb <MB>"));
    }

    #[test]
    fn test_evaluate_warning_when_available_between_100_and_150_percent() {
        // reserve=4096, 150% = 6144. available=5000 is between 4096..6144.
        let result =
            evaluate_memory_availability("test_tool", 5000, 1000, 6000, 32_000, 4096, None);
        // Should succeed (warning only, no error).
        assert!(result.is_ok(), "Should warn but not block: {result:?}");
    }

    #[test]
    fn test_evaluate_blocks_when_memavailable_below_reserve_even_with_swap() {
        let result =
            evaluate_memory_availability("test_tool", 3900, 4096, 7996, 32_000, 4096, None);
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
        let result =
            evaluate_memory_availability("test_tool", 7000, 1000, 8000, 32_000, 4096, None);
        assert!(result.is_ok(), "Should pass without warning: {result:?}");
    }

    #[test]
    fn test_evaluate_exact_boundary_at_reserve() {
        // Exactly at reserve — should pass (not strictly less than).
        let result =
            evaluate_memory_availability("test_tool", 4096, 1096, 5192, 32_000, 4096, None);
        assert!(result.is_ok(), "Exact reserve should pass: {result:?}");
    }

    #[test]
    fn test_evaluate_exact_boundary_at_warning_threshold() {
        // reserve=4096, 150% = 6144. available=6144 is exactly at warning threshold.
        let result =
            evaluate_memory_availability("test_tool", 6144, 1144, 7288, 32_000, 4096, None);
        // 6144 is NOT < 6144, so no warning.
        assert!(
            result.is_ok(),
            "Exact warning threshold should pass: {result:?}"
        );
    }

    #[test]
    fn test_evaluate_blocks_when_spawn_projection_exceeds_available_headroom() {
        let admission = SpawnMemoryAdmission {
            projected_spawn_mb: 8192,
            active_session_rss_mb: 2048,
            active_session_projected_mb: 4096,
            active_session_count: 1,
            sampled_session_count: 1,
        };

        let result =
            evaluate_memory_availability("codex", 10_000, 0, 10_000, 32_000, 4096, Some(admission));

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("host memory admission denied"));
        assert!(msg.contains("projected_spawn=8192MB"));
        assert!(msg.contains("Host admission uses physical MemAvailable only"));
        assert!(msg.contains("swap and combined memory are reported for diagnostics"));
        assert!(msg.contains("--memory-max-mb <MB>"));
        assert!(msg.contains("--min-free-memory-mb <MB>"));
        assert!(msg.contains("tools.<tool>.memory_max_mb"));
    }

    #[test]
    fn test_evaluate_blocks_when_active_projection_exceeds_host_safe_limit() {
        let admission = SpawnMemoryAdmission {
            projected_spawn_mb: 8192,
            active_session_rss_mb: 16_000,
            active_session_projected_mb: 20_000,
            active_session_count: 3,
            sampled_session_count: 2,
        };

        let result =
            evaluate_memory_availability("codex", 20_000, 0, 20_000, 32_000, 4096, Some(admission));

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("active-session memory admission denied"));
        assert!(msg.contains("projected_active=28192MB"));
        assert!(msg.contains("--memory-max-mb <MB>"));
        assert!(msg.contains("resources.memory_max_mb"));
    }

    #[test]
    fn test_evaluate_allows_safe_spawn_projection() {
        let admission = SpawnMemoryAdmission {
            projected_spawn_mb: 4096,
            active_session_rss_mb: 2048,
            active_session_projected_mb: 4096,
            active_session_count: 1,
            sampled_session_count: 1,
        };

        let result = evaluate_memory_availability(
            "claude-code",
            12_000,
            0,
            12_000,
            32_000,
            4096,
            Some(admission),
        );

        assert!(result.is_ok(), "safe projection should pass: {result:?}");
    }
}
