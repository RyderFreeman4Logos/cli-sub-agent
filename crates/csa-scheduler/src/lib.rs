//! Scheduler: tool selection (round-robin), session reuse, and 429 failover.

pub mod failover;
pub mod rate_limit;
pub mod rotation;
pub mod session_reuse;

pub use failover::{FailoverAction, decide_failover};
pub use rate_limit::{RateLimitDetected, detect_rate_limit};
pub use rotation::resolve_tier_tool_rotated;
pub use session_reuse::{ReuseCandidate, find_reusable_sessions};
