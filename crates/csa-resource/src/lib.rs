//! Resource-aware scheduling with simple threshold checks.

pub mod bwrap;
pub mod cgroup;
pub mod filesystem_sandbox;
pub mod guard;
pub mod isolation_plan;
#[cfg(target_os = "linux")]
pub mod landlock;
#[cfg(not(target_os = "linux"))]
#[path = "landlock_stub.rs"]
pub mod landlock;
pub mod memory_balloon;
pub mod memory_monitor;
pub mod rlimit;
pub mod sandbox;

pub use bwrap::{BwrapCommandBuilder, from_isolation_plan};
pub use cgroup::{
    CgroupScopeGuard, OrphanScope, SandboxConfig, cleanup_orphan_scopes, create_scope_command,
};
pub use filesystem_sandbox::{FilesystemCapability, detect_filesystem_capability};
pub use guard::{ResourceGuard, ResourceLimits};
pub use isolation_plan::{EnforcementMode, IsolationPlan, IsolationPlanBuilder};
pub use landlock::apply_landlock_rules;
pub use rlimit::apply_rlimits;
pub use sandbox::{ResourceCapability, detect_resource_capability};
