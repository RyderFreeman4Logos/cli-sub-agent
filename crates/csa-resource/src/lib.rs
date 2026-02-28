//! Resource-aware scheduling with P95 memory estimation.

pub mod cgroup;
pub mod guard;
pub mod memory_balloon;
pub mod monitor;
pub mod rlimit;
pub mod sandbox;
pub mod stats;

pub use cgroup::{
    CgroupScopeGuard, OrphanScope, SandboxConfig, cleanup_orphan_scopes, create_scope_command,
};
pub use guard::{ResourceGuard, ResourceLimits};
pub use monitor::MemoryMonitor;
pub use rlimit::{RssWatcher, apply_rlimits};
pub use sandbox::{SandboxCapability, detect_sandbox_capability};
pub use stats::UsageStats;
