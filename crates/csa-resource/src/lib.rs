//! Resource-aware scheduling with P95 memory estimation.

pub mod guard;
pub mod monitor;
pub mod stats;

pub use guard::{ResourceGuard, ResourceLimits};
pub use monitor::MemoryMonitor;
pub use stats::UsageStats;
