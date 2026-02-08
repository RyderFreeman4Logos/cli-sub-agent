//! Scheduler: tool selection (round-robin), session reuse, and 429 failover.

pub mod failover;
pub mod rate_limit;
pub mod rotation;
pub mod session_reuse;

pub use failover::{decide_failover, FailoverAction};
pub use rate_limit::{detect_rate_limit, RateLimitDetected};
pub use rotation::resolve_tier_tool_rotated;
pub use session_reuse::{find_reusable_sessions, ReuseCandidate};
