//! Scheduler: tool selection (round-robin), session reuse, seed management, and 429 failover.

pub mod failover;
pub mod rate_limit;
pub mod rotation;
pub mod seed_session;
pub mod session_reuse;

pub use failover::{FailoverAction, decide_failover};
pub use rate_limit::{RateLimitDetected, detect_rate_limit};
pub use rotation::resolve_tier_tool_rotated;
pub use seed_session::{
    SeedCandidate, evict_excess_seeds, find_seed_session, find_seed_session_for_native_fork,
    is_seed_valid,
};
pub use session_reuse::{ReuseCandidate, find_reusable_sessions};
