//! Resource-aware scheduling with simple threshold checks.

pub mod cgroup;
pub mod filesystem_sandbox;
pub mod guard;
pub mod isolation_plan;
pub mod memory_balloon;
pub mod rlimit;
pub mod sandbox;

pub use cgroup::{
    CgroupScopeGuard, OrphanScope, SandboxConfig, cleanup_orphan_scopes, create_scope_command,
};
pub use filesystem_sandbox::{FilesystemCapability, detect_filesystem_capability};
pub use guard::{ResourceGuard, ResourceLimits};
pub use isolation_plan::{EnforcementMode, IsolationPlan, IsolationPlanBuilder};
pub use rlimit::apply_rlimits;
pub use sandbox::{ResourceCapability, detect_resource_capability};
