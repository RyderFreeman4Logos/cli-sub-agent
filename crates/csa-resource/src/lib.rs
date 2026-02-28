//! Resource-aware scheduling with simple threshold checks.

pub mod cgroup;
pub mod guard;
pub mod memory_balloon;
pub mod rlimit;
pub mod sandbox;

pub use cgroup::{
    CgroupScopeGuard, OrphanScope, SandboxConfig, cleanup_orphan_scopes, create_scope_command,
};
pub use guard::{ResourceGuard, ResourceLimits};
pub use rlimit::{RssWatcher, apply_rlimits};
pub use sandbox::{SandboxCapability, detect_sandbox_capability};
