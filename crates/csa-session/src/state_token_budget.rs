//! Token usage and budget accounting types for session state.
//!
//! Split out of `state.rs` via the `#[path]` sibling idiom so each module
//! stays under the per-module token budget. Re-exported through `state.rs`
//! so the public paths `csa_session::state::{TokenUsage, TokenBudget}` and
//! the crate facade remain stable.

use serde::{Deserialize, Serialize};

/// Token usage tracking for AI tool execution
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Input tokens consumed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,

    /// Output tokens generated
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,

    /// Output tokens spent on provider-side reasoning, when reported.
    ///
    /// This is a subset/detail of output tokens for providers that expose it.
    /// Older sessions and providers that do not report reasoning usage leave
    /// it unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_output_tokens: Option<u64>,

    /// Total tokens (input + output)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,

    /// Estimated cost in USD
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_cost_usd: Option<f64>,

    /// Cache-read/cached input tokens reported by the provider.
    ///
    /// When present, this is the portion of `input_tokens` served from the
    /// provider's prompt cache. Older sessions and some tools may not
    /// populate this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
}

impl TokenUsage {
    /// Input tokens that were not served from the provider prompt cache.
    ///
    /// Returns `None` unless both total input and cached/cache-read input are
    /// known. Saturates at zero for defensive compatibility with provider
    /// anomalies that report cached input above total input.
    pub fn uncached_input_tokens(&self) -> Option<u64> {
        Some(
            self.input_tokens?
                .saturating_sub(self.cache_read_input_tokens?),
        )
    }

    /// Ratio of cache-read input tokens to total input tokens (`cache_read / input_tokens`).
    ///
    /// Returns `None` when either field is missing or when `input_tokens` is
    /// zero (no meaningful denominator).
    pub fn cache_read_ratio(&self) -> Option<f64> {
        let cache_read = self.cache_read_input_tokens? as f64;
        let total_input = self.input_tokens? as f64;
        if total_input == 0.0 {
            return None;
        }
        Some(cache_read / total_input)
    }

    /// Ratio of cache-read input tokens to total input tokens (`cache_read / input_tokens`).
    ///
    /// Returns `None` when either field is missing or when `input_tokens` is
    /// zero (no meaningful denominator).
    pub fn cache_hit_ratio(&self) -> Option<f64> {
        self.cache_read_ratio()
    }
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
