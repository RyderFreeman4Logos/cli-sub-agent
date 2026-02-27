//! Session state types

use crate::output_section::ReturnPacketRef;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};

const FORK_CALL_RATE_LIMIT_MAX: usize = 10;
const FORK_CALL_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);

/// Meta-session state representing a logical work session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaSessionState {
    /// ULID identifier (26 characters, Crockford Base32)
    pub meta_session_id: String,

    /// Human-readable description (optional)
    pub description: Option<String>,

    /// Absolute path to the project directory
    pub project_path: String,

    /// Git branch at session creation time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,

    /// When this session was created
    pub created_at: DateTime<Utc>,

    /// When this session was last accessed
    pub last_accessed: DateTime<Utc>,

    /// Genealogy information (parent, depth)
    #[serde(default)]
    pub genealogy: Genealogy,

    /// Tool-specific state (provider session IDs, etc.)
    #[serde(default)]
    pub tools: HashMap<String, ToolState>,

    /// Context compaction status
    #[serde(default)]
    pub context_status: ContextStatus,

    /// Cumulative token usage across all tools in this session
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_token_usage: Option<TokenUsage>,

    /// Lifecycle phase of this session.
    #[serde(default)]
    pub phase: SessionPhase,

    /// Context about the task this session is working on.
    #[serde(default)]
    pub task_context: TaskContext,

    /// Number of execution turns in this session.
    #[serde(default)]
    pub turn_count: u32,

    /// Token budget tracking (allocated, used, remaining).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<TokenBudget>,

    /// Resource sandbox telemetry for this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_info: Option<SandboxInfo>,

    /// Why the last run terminated early (e.g. sigint, sigterm, idle_timeout).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub termination_reason: Option<String>,

    /// Whether this session is a seed candidate for future fork-from-seed.
    #[serde(default)]
    pub is_seed_candidate: bool,

    /// Git HEAD commit hash at session creation time.
    /// Used for seed invalidation: if HEAD changed, the seed is stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_head_at_creation: Option<String>,

    /// Reference to the latest child return packet captured via fork-call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_return_packet: Option<ReturnPacketRef>,

    /// In-memory fork-call timestamps for simple per-session rate limiting.
    ///
    /// This is intentionally runtime-only and is not persisted to state.toml.
    #[serde(skip)]
    pub fork_call_timestamps: Vec<Instant>,
}

/// Lightweight telemetry about the resource sandbox applied to a session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SandboxInfo {
    /// Sandbox isolation mode used: "cgroup", "rlimit", or "none".
    pub mode: String,
    /// Memory limit applied (MB), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_max_mb: Option<u64>,
}

/// Genealogy tracking for session parent-child relationships
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Genealogy {
    /// Parent session ID (None for root sessions)
    pub parent_session_id: Option<String>,

    /// Depth in the genealogy tree (0 for root sessions)
    pub depth: u32,

    /// The CSA session that was forked FROM (distinguishes fork-child from spawn-child).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_of_session_id: Option<String>,

    /// Provider-level session ID used for the fork (e.g., Claude Code's internal session ID).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_provider_session_id: Option<String>,
    // Note: Children are discovered dynamically via scanning, not stored here
}

impl Genealogy {
    /// Returns `true` if this session was created by forking another session.
    pub fn is_fork(&self) -> bool {
        self.fork_of_session_id.is_some()
    }

    /// Returns the CSA session ID this session was forked from, if any.
    pub fn fork_source(&self) -> Option<&str> {
        self.fork_of_session_id.as_deref()
    }
}

/// Per-tool state within a session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolState {
    /// Provider-specific session ID (e.g., Codex thread_id, Gemini session)
    /// None on first run before provider session is created
    pub provider_session_id: Option<String>,

    /// Summary of the last action performed by this tool
    pub last_action_summary: String,

    /// Exit code of the last tool invocation
    pub last_exit_code: i32,

    /// When this tool state was last updated
    pub updated_at: DateTime<Utc>,

    /// Token usage for this tool in this session
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<TokenUsage>,
}

/// Token usage tracking for AI tool execution
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Input tokens consumed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,

    /// Output tokens generated
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,

    /// Total tokens (input + output)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,

    /// Estimated cost in USD
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_cost_usd: Option<f64>,
}

/// Token budget for session-level resource governance.
///
/// Tracks how many tokens were allocated (from tier or config) and how many
/// have been consumed. Soft threshold triggers a warning; hard threshold
/// blocks further execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TokenBudget {
    /// Total tokens allocated for this session (from tier config).
    pub allocated: u64,

    /// Tokens consumed so far.
    #[serde(default)]
    pub used: u64,

    /// Percentage threshold for soft warning (default 75).
    #[serde(default = "default_soft_threshold_pct")]
    pub soft_threshold_pct: u32,

    /// Percentage threshold for hard block (default 100).
    #[serde(default = "default_hard_threshold_pct")]
    pub hard_threshold_pct: u32,

    /// Optional max turns limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
}

fn default_soft_threshold_pct() -> u32 {
    75
}

fn default_hard_threshold_pct() -> u32 {
    100
}

impl TokenBudget {
    /// Create a new budget with the given allocation.
    pub fn new(allocated: u64) -> Self {
        Self {
            allocated,
            used: 0,
            soft_threshold_pct: default_soft_threshold_pct(),
            hard_threshold_pct: default_hard_threshold_pct(),
            max_turns: None,
        }
    }

    /// Remaining tokens before hard threshold.
    pub fn remaining(&self) -> u64 {
        let hard_limit = self.hard_limit();
        hard_limit.saturating_sub(self.used)
    }

    /// The absolute token count for the hard threshold.
    pub fn hard_limit(&self) -> u64 {
        (self.allocated as u128 * self.hard_threshold_pct as u128 / 100) as u64
    }

    /// The absolute token count for the soft warning threshold.
    pub fn soft_limit(&self) -> u64 {
        (self.allocated as u128 * self.soft_threshold_pct as u128 / 100) as u64
    }

    /// Usage percentage (0-100+).
    pub fn usage_pct(&self) -> u32 {
        if self.allocated == 0 {
            return 0;
        }
        ((self.used as u128 * 100) / self.allocated as u128) as u32
    }

    /// Whether the soft warning threshold has been crossed.
    pub fn is_soft_exceeded(&self) -> bool {
        self.used >= self.soft_limit()
    }

    /// Whether the hard block threshold has been crossed.
    pub fn is_hard_exceeded(&self) -> bool {
        self.used >= self.hard_limit()
    }

    /// Record token usage from an execution turn.
    pub fn record_usage(&mut self, tokens: u64) {
        self.used = self.used.saturating_add(tokens);
    }

    /// Whether the max turns limit has been reached.
    pub fn is_turns_exceeded(&self, turn_count: u32) -> bool {
        self.max_turns.is_some_and(|max| turn_count >= max)
    }
}

/// Context compaction status tracking
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextStatus {
    /// Whether the context has been compacted
    pub is_compacted: bool,

    /// When the context was last compacted (if ever)
    pub last_compacted_at: Option<DateTime<Utc>>,
}

/// Events that trigger session phase transitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhaseEvent {
    /// Context compression completed successfully.
    Compressed,
    /// Session is being resumed for a new task.
    Resumed,
    /// Session should be retired (by GC aging or explicit request).
    Retired,
}

/// Session lifecycle phase.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionPhase {
    /// Currently executing or holding active context.
    #[default]
    Active,
    /// Compacted and ready to accept a new task.
    Available,
    /// Lifecycle complete — no longer eligible for reuse.
    Retired,
}

impl SessionPhase {
    /// Attempt a phase transition driven by `event`.
    ///
    /// Returns the new phase on success, or an error description for invalid
    /// transitions. The state machine is intentionally simple:
    ///
    /// ```text
    ///   Active  --Compressed--> Available
    ///   Active  --Retired-----> Retired
    ///   Available --Resumed---> Active
    ///   Available --Retired---> Retired
    /// ```
    ///
    /// All other combinations are invalid.
    pub fn transition(&self, event: &PhaseEvent) -> Result<SessionPhase, String> {
        match (self, event) {
            (SessionPhase::Active, PhaseEvent::Compressed) => Ok(SessionPhase::Available),
            (SessionPhase::Active, PhaseEvent::Retired) => Ok(SessionPhase::Retired),
            (SessionPhase::Available, PhaseEvent::Resumed) => Ok(SessionPhase::Active),
            (SessionPhase::Available, PhaseEvent::Retired) => Ok(SessionPhase::Retired),
            (current, event) => Err(format!(
                "invalid phase transition: {:?} + {:?}",
                current, event
            )),
        }
    }
}

impl MetaSessionState {
    /// Apply a lifecycle event to this session and update `phase` in-place.
    pub fn apply_phase_event(&mut self, event: PhaseEvent) -> Result<(), String> {
        let new_phase = self.phase.transition(&event)?;
        self.phase = new_phase;
        Ok(())
    }

    /// Record a fork-call attempt and enforce a per-session sliding-window rate limit.
    ///
    /// Limit: at most 10 fork-calls per 60 seconds.
    pub fn record_fork_call_attempt(&mut self, now: Instant) -> Result<(), String> {
        self.fork_call_timestamps.retain(|ts| {
            now.checked_duration_since(*ts)
                .is_some_and(|elapsed| elapsed < FORK_CALL_RATE_LIMIT_WINDOW)
        });

        if self.fork_call_timestamps.len() >= FORK_CALL_RATE_LIMIT_MAX {
            return Err(format!(
                "fork-call rate limit exceeded: max {} per {}s",
                FORK_CALL_RATE_LIMIT_MAX,
                FORK_CALL_RATE_LIMIT_WINDOW.as_secs()
            ));
        }

        self.fork_call_timestamps.push(now);
        Ok(())
    }
}

impl std::fmt::Display for SessionPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionPhase::Active => write!(f, "active"),
            SessionPhase::Available => write!(f, "available"),
            SessionPhase::Retired => write!(f, "retired"),
        }
    }
}

/// Lightweight context about what the session was doing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskContext {
    /// Kind of task (e.g. "review", "implement", "fix", "default").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_type: Option<String>,
    /// Which tier this session was allocated from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_name: Option<String>,
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
