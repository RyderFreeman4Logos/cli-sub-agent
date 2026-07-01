use std::path::Path;

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

/// Upper-bound inputs for a retry after host-memory admission denial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryAdmissionRetryBounds {
    /// Upper bound from physical MemAvailable after preserving the configured reserve.
    pub physical_upper_mb: u64,
    /// Upper bound from already-active CSA session pressure, when total RAM is known.
    pub active_session_upper_mb: Option<u64>,
    /// Effective upper bound that satisfies both physical and active-session gates.
    pub combined_upper_mb: u64,
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
    pub retry_physical_upper_mb: Option<u64>,
    pub retry_active_session_upper_mb: Option<u64>,
    pub retry_combined_upper_mb: Option<u64>,
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
            retry_physical_upper_mb: snapshot.retry_bounds.map(|bounds| bounds.physical_upper_mb),
            retry_active_session_upper_mb: snapshot
                .retry_bounds
                .and_then(|bounds| bounds.active_session_upper_mb),
            retry_combined_upper_mb: snapshot.retry_bounds.map(|bounds| bounds.combined_upper_mb),
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
    retry_bounds: Option<MemoryAdmissionRetryBounds>,
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

        let available_phys_bytes = effective_available_memory_bytes(self.sys.available_memory());
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
const CGROUP_ROOT: &str = "/sys/fs/cgroup";

fn effective_available_memory_bytes(available_phys_bytes: u64) -> u64 {
    available_phys_bytes.min(cgroup_available_memory_bytes().unwrap_or(u64::MAX))
}

#[cfg(test)]
fn effective_available_memory_bytes_at(available_phys_bytes: u64, cgroup_root: &Path) -> u64 {
    available_phys_bytes.min(cgroup_available_memory_bytes_at(cgroup_root).unwrap_or(u64::MAX))
}

fn cgroup_available_memory_bytes() -> Option<u64> {
    cgroup_available_memory_bytes_at(Path::new(CGROUP_ROOT))
}

fn cgroup_available_memory_bytes_at(cgroup_root: &Path) -> Option<u64> {
    if let Some(available) = cgroup_v2_available_memory_bytes(cgroup_root) {
        return available;
    }
    cgroup_v1_available_memory_bytes(cgroup_root)
}

fn cgroup_v2_available_memory_bytes(cgroup_root: &Path) -> Option<Option<u64>> {
    let limit = read_cgroup_limit_bytes(&cgroup_root.join("memory.max"))?;
    let current = read_cgroup_usage_bytes(&cgroup_root.join("memory.current"))?;
    Some(limit.map(|limit| limit.saturating_sub(current)))
}

fn cgroup_v1_available_memory_bytes(cgroup_root: &Path) -> Option<u64> {
    let memory_root = cgroup_root.join("memory");
    let limit = read_cgroup_limit_bytes(&memory_root.join("memory.limit_in_bytes"))??;
    let current = read_cgroup_usage_bytes(&memory_root.join("memory.usage_in_bytes"))?;
    Some(limit.saturating_sub(current))
}

fn read_cgroup_limit_bytes(path: &Path) -> Option<Option<u64>> {
    let value = std::fs::read_to_string(path).ok()?;
    let trimmed = value.trim();
    if trimmed == "max" {
        return Some(None);
    }
    trimmed.parse::<u64>().ok().map(Some)
}

fn read_cgroup_usage_bytes(path: &Path) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

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
    let admission_retry = admission.map(|admission| {
        let retry_bounds = retry_bounds_for(available_phys_mb, total_ram_mb, reserve_mb, admission);
        (
            admission,
            retry_bounds,
            format_retry_upper_bound(retry_bounds),
        )
    });
    let base_snapshot = MemoryAdmissionSnapshot {
        available_phys_mb,
        available_swap_mb,
        available_combined_mb,
        total_ram_mb,
        reserve_mb,
        projected_spawn_mb: admission.map(|admission| admission.projected_spawn_mb),
        active_session_rss_mb: admission.map(|admission| admission.active_session_rss_mb),
        active_session_projected_mb: admission
            .map(|admission| admission.active_session_projected_mb),
        active_session_count: admission.map(|admission| admission.active_session_count),
        sampled_session_count: admission.map(|admission| admission.sampled_session_count),
        retry_bounds: admission_retry
            .as_ref()
            .map(|(_, retry_bounds, _)| *retry_bounds),
        ..Default::default()
    };

    if available_phys_mb < reserve_mb {
        let retry_note = admission_retry
            .as_ref()
            .map(|(_, _, note)| {
                format!(
                    " Pre-exec memory admission is infrastructure/session-unavailable before \
                     provider launch, not a product/test/review failure. {note}"
                )
            })
            .unwrap_or_default();
        let message = format!(
            "CSA: low memory — available={available_phys_mb}MB < required={reserve_mb}MB. \
             Refusing to spawn tool scopes. actual_available_mb={available_phys_mb} \
             required_mb={reserve_mb} physical_available_mb={available_phys_mb} \
             swap_available_mb={available_swap_mb} combined_available_mb={available_combined_mb}{retry_note}"
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

    if let Some((admission, retry_bounds, retry_note)) = admission_retry
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
                 combined_available_mb={available_combined_mb}. Pre-exec memory admission is \
                 infrastructure/session-unavailable before provider launch, not a \
                 product/test/review failure. {retry_note}",
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
                     to lower the reserve. Choose a memory_max_mb inside the printed retry \
                     upper bound and the role/tool soft-limit floor; if no such window exists, \
                     wait/free memory or reduce active CSA-session pressure. Persistent config \
                     keys: resources.memory_max_mb, tools.<tool>.memory_max_mb, \
                     resources.min_free_memory_mb."
                ),
                MemoryAdmissionKind::HostSpawn,
                MemoryAdmissionSnapshot {
                    required_available_mb: Some(required_available_mb),
                    projected_spawn_mb: Some(admission.projected_spawn_mb),
                    active_session_rss_mb: Some(admission.active_session_rss_mb),
                    active_session_projected_mb: Some(admission.active_session_projected_mb),
                    active_session_count: Some(admission.active_session_count),
                    sampled_session_count: Some(admission.sampled_session_count),
                    retry_bounds: Some(retry_bounds),
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
                 projected_spawn_mb={projected_spawn_mb} available_mb={available_phys_mb}. \
                 Pre-exec memory admission is infrastructure/session-unavailable before provider \
                 launch, not a product/test/review failure. {retry_note}",
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
                     Choose a memory_max_mb inside the printed retry upper bound and the \
                     role/tool soft-limit floor; if no such window exists, wait for active CSA \
                     sessions to finish or use a different configured tool. Persistent config \
                     keys: resources.memory_max_mb or tools.<tool>.memory_max_mb."
                ),
                MemoryAdmissionKind::ActiveSession,
                MemoryAdmissionSnapshot {
                    required_available_mb: Some(host_safe_limit_mb),
                    projected_spawn_mb: Some(admission.projected_spawn_mb),
                    active_session_rss_mb: Some(admission.active_session_rss_mb),
                    active_session_projected_mb: Some(admission.active_session_projected_mb),
                    active_session_count: Some(admission.active_session_count),
                    sampled_session_count: Some(admission.sampled_session_count),
                    retry_bounds: Some(retry_bounds),
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

fn retry_bounds_for(
    available_phys_mb: u64,
    total_ram_mb: u64,
    reserve_mb: u64,
    admission: SpawnMemoryAdmission,
) -> MemoryAdmissionRetryBounds {
    let physical_upper_mb = available_phys_mb.saturating_sub(reserve_mb);
    let active_session_upper_mb = active_session_retry_upper_mb(total_ram_mb, admission);
    let combined_upper_mb = active_session_upper_mb.map_or(physical_upper_mb, |active_upper| {
        physical_upper_mb.min(active_upper)
    });

    MemoryAdmissionRetryBounds {
        physical_upper_mb,
        active_session_upper_mb,
        combined_upper_mb,
    }
}

fn active_session_retry_upper_mb(
    total_ram_mb: u64,
    admission: SpawnMemoryAdmission,
) -> Option<u64> {
    let host_safe_limit_mb = total_ram_mb.saturating_mul(ACTIVE_SESSION_SAFE_FRACTION_NUM)
        / ACTIVE_SESSION_SAFE_FRACTION_DEN;
    (host_safe_limit_mb > 0)
        .then(|| host_safe_limit_mb.saturating_sub(admission.active_session_projected_mb))
}

fn format_retry_upper_bound(bounds: MemoryAdmissionRetryBounds) -> String {
    let active_upper = bounds
        .active_session_upper_mb
        .map(|upper| format!("{upper}MB"))
        .unwrap_or_else(|| "unknown".to_string());
    format!(
        "Retry upper bound: memory_max_mb <= {combined}MB \
         (physical/reserve upper={physical}MB; active-session upper={active}).",
        combined = bounds.combined_upper_mb,
        physical = bounds.physical_upper_mb,
        active = active_upper,
    )
}

#[cfg(test)]
#[path = "guard_tests.rs"]
mod tests;
