//! Session state types

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Meta-session state representing a logical work session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaSessionState {
    /// ULID identifier (26 characters, Crockford Base32)
    pub meta_session_id: String,

    /// Human-readable description (optional)
    pub description: Option<String>,

    /// Absolute path to the project directory
    pub project_path: String,

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
}

/// Genealogy tracking for session parent-child relationships
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Genealogy {
    /// Parent session ID (None for root sessions)
    pub parent_session_id: Option<String>,

    /// Depth in the genealogy tree (0 for root sessions)
    pub depth: u32,
    // Note: Children are discovered dynamically via scanning, not stored here
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
mod tests {
    use super::*;

    // ── Valid transitions ────────────────────────────────────────────

    #[test]
    fn test_active_compressed_becomes_available() {
        let phase = SessionPhase::Active;
        assert_eq!(
            phase.transition(&PhaseEvent::Compressed),
            Ok(SessionPhase::Available)
        );
    }

    #[test]
    fn test_active_retired_becomes_retired() {
        let phase = SessionPhase::Active;
        assert_eq!(
            phase.transition(&PhaseEvent::Retired),
            Ok(SessionPhase::Retired)
        );
    }

    #[test]
    fn test_available_resumed_becomes_active() {
        let phase = SessionPhase::Available;
        assert_eq!(
            phase.transition(&PhaseEvent::Resumed),
            Ok(SessionPhase::Active)
        );
    }

    #[test]
    fn test_available_retired_becomes_retired() {
        let phase = SessionPhase::Available;
        assert_eq!(
            phase.transition(&PhaseEvent::Retired),
            Ok(SessionPhase::Retired)
        );
    }

    // ── Invalid transitions ─────────────────────────────────────────

    #[test]
    fn test_active_resumed_is_invalid() {
        let phase = SessionPhase::Active;
        assert!(phase.transition(&PhaseEvent::Resumed).is_err());
    }

    #[test]
    fn test_available_compressed_is_invalid() {
        let phase = SessionPhase::Available;
        assert!(phase.transition(&PhaseEvent::Compressed).is_err());
    }

    #[test]
    fn test_retired_compressed_is_invalid() {
        let phase = SessionPhase::Retired;
        assert!(phase.transition(&PhaseEvent::Compressed).is_err());
    }

    #[test]
    fn test_retired_resumed_is_invalid() {
        let phase = SessionPhase::Retired;
        assert!(phase.transition(&PhaseEvent::Resumed).is_err());
    }

    #[test]
    fn test_retired_retired_is_invalid() {
        let phase = SessionPhase::Retired;
        assert!(phase.transition(&PhaseEvent::Retired).is_err());
    }

    // ── Display ─────────────────────────────────────────────────────

    #[test]
    fn test_display() {
        assert_eq!(SessionPhase::Active.to_string(), "active");
        assert_eq!(SessionPhase::Available.to_string(), "available");
        assert_eq!(SessionPhase::Retired.to_string(), "retired");
    }

    // ── Round-trip: Active → Available → Active ─────────────────────

    #[test]
    fn test_round_trip_active_available_active() {
        let phase = SessionPhase::Active;
        let available = phase.transition(&PhaseEvent::Compressed).unwrap();
        assert_eq!(available, SessionPhase::Available);
        let active_again = available.transition(&PhaseEvent::Resumed).unwrap();
        assert_eq!(active_again, SessionPhase::Active);
    }

    // ── Serde round-trip ───────────────────────────────────────────

    /// Wrapper struct to test enum serialization (TOML can't serialize bare enums).
    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct PhaseWrapper {
        phase: SessionPhase,
    }

    #[test]
    fn test_session_phase_serde_roundtrip() {
        for phase in [
            SessionPhase::Active,
            SessionPhase::Available,
            SessionPhase::Retired,
        ] {
            let wrapper = PhaseWrapper {
                phase: phase.clone(),
            };
            let serialized = toml::to_string(&wrapper).expect("Serialize should succeed");
            let deserialized: PhaseWrapper =
                toml::from_str(&serialized).expect("Deserialize should succeed");
            assert_eq!(deserialized.phase, phase);
        }
    }

    #[test]
    fn test_session_phase_serde_snake_case() {
        // Verify rename_all = "snake_case" produces expected strings
        let active_toml = toml::to_string(&PhaseWrapper {
            phase: SessionPhase::Active,
        })
        .unwrap();
        assert!(active_toml.contains("active"));

        let available_toml = toml::to_string(&PhaseWrapper {
            phase: SessionPhase::Available,
        })
        .unwrap();
        assert!(available_toml.contains("available"));

        let retired_toml = toml::to_string(&PhaseWrapper {
            phase: SessionPhase::Retired,
        })
        .unwrap();
        assert!(retired_toml.contains("retired"));
    }

    // ── Error message content ──────────────────────────────────────

    #[test]
    fn test_invalid_transition_error_contains_states() {
        let err = SessionPhase::Retired
            .transition(&PhaseEvent::Compressed)
            .unwrap_err();
        assert!(
            err.contains("Retired"),
            "Error should mention the current phase"
        );
        assert!(err.contains("Compressed"), "Error should mention the event");
    }

    // ── Default phase ──────────────────────────────────────────────

    #[test]
    fn test_default_phase_is_active() {
        let phase: SessionPhase = Default::default();
        assert_eq!(phase, SessionPhase::Active);
    }

    // ── MetaSessionState TOML round-trip ───────────────────────────

    #[test]
    fn test_meta_session_state_toml_roundtrip() {
        let now = chrono::Utc::now();
        let state = MetaSessionState {
            meta_session_id: ulid::Ulid::new().to_string(),
            description: Some("Round-trip test".to_string()),
            project_path: "/tmp/test".to_string(),
            created_at: now,
            last_accessed: now,
            genealogy: Genealogy {
                parent_session_id: None,
                depth: 0,
            },
            tools: HashMap::new(),
            context_status: ContextStatus::default(),
            total_token_usage: None,
            phase: SessionPhase::Available,
            task_context: TaskContext {
                task_type: Some("review".to_string()),
                tier_name: Some("quick".to_string()),
            },
            turn_count: 0,
            token_budget: None,
        };

        let toml_str = toml::to_string_pretty(&state).expect("Serialize should succeed");
        let loaded: MetaSessionState =
            toml::from_str(&toml_str).expect("Deserialize should succeed");

        assert_eq!(loaded.meta_session_id, state.meta_session_id);
        assert_eq!(loaded.description, state.description);
        assert_eq!(loaded.phase, SessionPhase::Available);
        assert_eq!(loaded.task_context.task_type, Some("review".to_string()));
        assert_eq!(loaded.task_context.tier_name, Some("quick".to_string()));
    }

    // ── Retired is terminal ────────────────────────────────────────

    #[test]
    fn test_retired_is_terminal_for_all_events() {
        let retired = SessionPhase::Retired;
        assert!(retired.transition(&PhaseEvent::Compressed).is_err());
        assert!(retired.transition(&PhaseEvent::Resumed).is_err());
        assert!(retired.transition(&PhaseEvent::Retired).is_err());
    }

    // ── TokenBudget ──────────────────────────────────────────────────

    #[test]
    fn test_token_budget_new_defaults() {
        let budget = TokenBudget::new(100_000);
        assert_eq!(budget.allocated, 100_000);
        assert_eq!(budget.used, 0);
        assert_eq!(budget.soft_threshold_pct, 75);
        assert_eq!(budget.hard_threshold_pct, 100);
        assert_eq!(budget.max_turns, None);
    }

    #[test]
    fn test_token_budget_remaining() {
        let mut budget = TokenBudget::new(100_000);
        assert_eq!(budget.remaining(), 100_000);
        budget.record_usage(30_000);
        assert_eq!(budget.remaining(), 70_000);
        budget.record_usage(70_000);
        assert_eq!(budget.remaining(), 0);
    }

    #[test]
    fn test_token_budget_remaining_saturates() {
        let mut budget = TokenBudget::new(100_000);
        budget.record_usage(200_000);
        assert_eq!(budget.remaining(), 0);
    }

    #[test]
    fn test_token_budget_usage_pct() {
        let mut budget = TokenBudget::new(100_000);
        assert_eq!(budget.usage_pct(), 0);
        budget.record_usage(50_000);
        assert_eq!(budget.usage_pct(), 50);
        budget.record_usage(25_000);
        assert_eq!(budget.usage_pct(), 75);
        budget.record_usage(25_000);
        assert_eq!(budget.usage_pct(), 100);
    }

    #[test]
    fn test_token_budget_usage_pct_zero_allocated() {
        let budget = TokenBudget::new(0);
        assert_eq!(budget.usage_pct(), 0);
    }

    #[test]
    fn test_token_budget_soft_threshold() {
        let mut budget = TokenBudget::new(100_000);
        budget.record_usage(74_999);
        assert!(!budget.is_soft_exceeded());
        budget.record_usage(1);
        assert!(budget.is_soft_exceeded());
    }

    #[test]
    fn test_token_budget_hard_threshold() {
        let mut budget = TokenBudget::new(100_000);
        budget.record_usage(99_999);
        assert!(!budget.is_hard_exceeded());
        budget.record_usage(1);
        assert!(budget.is_hard_exceeded());
    }

    #[test]
    fn test_token_budget_custom_thresholds() {
        let mut budget = TokenBudget::new(100_000);
        budget.soft_threshold_pct = 50;
        budget.hard_threshold_pct = 80;

        budget.record_usage(49_999);
        assert!(!budget.is_soft_exceeded());
        budget.record_usage(1);
        assert!(budget.is_soft_exceeded());
        assert!(!budget.is_hard_exceeded());

        budget.record_usage(29_999);
        assert!(!budget.is_hard_exceeded());
        budget.record_usage(1);
        assert!(budget.is_hard_exceeded());
    }

    #[test]
    fn test_token_budget_turns_exceeded() {
        let mut budget = TokenBudget::new(100_000);
        assert!(!budget.is_turns_exceeded(10));

        budget.max_turns = Some(5);
        assert!(!budget.is_turns_exceeded(4));
        assert!(budget.is_turns_exceeded(5));
        assert!(budget.is_turns_exceeded(10));
    }

    #[test]
    fn test_token_budget_record_usage_saturates() {
        let mut budget = TokenBudget::new(100_000);
        budget.record_usage(u64::MAX);
        assert_eq!(budget.used, u64::MAX);
        budget.record_usage(1);
        assert_eq!(budget.used, u64::MAX); // saturating add
    }

    #[test]
    fn test_token_budget_serde_roundtrip() {
        let mut budget = TokenBudget::new(200_000);
        budget.used = 50_000;
        budget.max_turns = Some(10);

        #[derive(Debug, Serialize, Deserialize, PartialEq)]
        struct BudgetWrapper {
            budget: TokenBudget,
        }

        let wrapper = BudgetWrapper {
            budget: budget.clone(),
        };
        let serialized = toml::to_string(&wrapper).expect("Serialize should succeed");
        let deserialized: BudgetWrapper =
            toml::from_str(&serialized).expect("Deserialize should succeed");
        assert_eq!(deserialized.budget, budget);
    }

    #[test]
    fn test_token_budget_serde_defaults() {
        // Deserialize with missing optional fields — serde defaults should fill them
        let toml_str = r#"
            [budget]
            allocated = 100000
        "#;

        #[derive(Debug, Deserialize)]
        struct BudgetWrapper {
            budget: TokenBudget,
        }

        let wrapper: BudgetWrapper = toml::from_str(toml_str).expect("Deserialize should succeed");
        assert_eq!(wrapper.budget.allocated, 100_000);
        assert_eq!(wrapper.budget.used, 0);
        assert_eq!(wrapper.budget.soft_threshold_pct, 75);
        assert_eq!(wrapper.budget.hard_threshold_pct, 100);
        assert_eq!(wrapper.budget.max_turns, None);
    }

    #[test]
    fn test_meta_session_state_with_budget_roundtrip() {
        let now = chrono::Utc::now();
        let mut budget = TokenBudget::new(150_000);
        budget.used = 30_000;
        budget.max_turns = Some(8);

        let state = MetaSessionState {
            meta_session_id: ulid::Ulid::new().to_string(),
            description: Some("Budget test".to_string()),
            project_path: "/tmp/test".to_string(),
            created_at: now,
            last_accessed: now,
            genealogy: Genealogy {
                parent_session_id: None,
                depth: 0,
            },
            tools: HashMap::new(),
            context_status: ContextStatus::default(),
            total_token_usage: None,
            phase: SessionPhase::Active,
            task_context: TaskContext::default(),
            turn_count: 3,
            token_budget: Some(budget.clone()),
        };

        let toml_str = toml::to_string_pretty(&state).expect("Serialize should succeed");
        let loaded: MetaSessionState =
            toml::from_str(&toml_str).expect("Deserialize should succeed");

        assert_eq!(loaded.turn_count, 3);
        assert_eq!(loaded.token_budget, Some(budget));
    }
}
