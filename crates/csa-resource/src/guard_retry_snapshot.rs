use super::*;

/// Fresh host-memory inputs used to render a retry-feasibility interval.
///
/// The snapshot reuses the same physical/reserve and active-session bound rules
/// as pre-spawn host-memory admission, but it is available when another
/// pre-exec admission (such as a soft-limit floor) failed first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryAdmissionRetrySnapshot {
    /// Effective physical MemAvailable after enclosing cgroup limits are applied.
    pub available_phys_mb: u64,
    /// Configured physical-memory reserve retained by a retry.
    pub reserve_mb: u64,
    /// Active-session projection used for the retry calculation.
    pub admission: SpawnMemoryAdmission,
    /// Unified physical/reserve and active-session retry bounds.
    pub retry_bounds: MemoryAdmissionRetryBounds,
}

impl ResourceGuard {
    /// Sample the unified retry bounds for a prospective spawn without denying it.
    ///
    /// This is intended for a different pre-exec guard that has already rejected
    /// the spawn but still needs to render an honest lower/upper retry interval.
    pub fn memory_admission_retry_snapshot(
        &mut self,
        admission: SpawnMemoryAdmission,
    ) -> MemoryAdmissionRetrySnapshot {
        self.sys.refresh_memory();

        let available_phys_mb =
            effective_available_memory_bytes(self.sys.available_memory()) / 1024 / 1024;
        let total_ram_mb = self.sys.total_memory() / 1024 / 1024;
        let reserve_mb = self.limits.min_free_memory_mb;
        let retry_bounds = retry_bounds_for(available_phys_mb, total_ram_mb, reserve_mb, admission);

        MemoryAdmissionRetrySnapshot {
            available_phys_mb,
            reserve_mb,
            admission,
            retry_bounds,
        }
    }
}
