/// Incrementally accumulated metadata from streamed transport events.
#[derive(Debug, Clone, Default)]
pub struct StreamingMetadata {
    /// Total number of events seen across the entire prompt turn, including dropped events.
    pub total_events_count: usize,
    /// Number of agent conversation turns observed in this prompt run.
    ///
    /// Counted by transports as the number of `AgentMessage` events emitted by
    /// the underlying tool. A single `csa run` invocation may span many turns
    /// when the agent reads tool results, deliberates, and responds again.
    /// Transports that do not parse streaming events leave this at `0`; callers
    /// must treat `0` as "unknown" and fall back to a `+= 1` increment for
    /// `MetaSessionState.turn_count` to preserve the legacy counting contract.
    pub turn_count: u32,
    /// Whether any `ToolCallStarted` event was observed.
    pub has_tool_calls: bool,
    /// Whether any execute `ToolCallStarted` event was observed.
    pub has_execute_tool_calls: bool,
    /// Whether a `--no-verify` or `-n` git commit command was observed.
    pub has_no_verify_commit: bool,
    /// Whether any `PlanUpdate` event was observed.
    pub has_plan_updates: bool,
    /// Tail of execute command titles observed during the prompt turn.
    pub extracted_commands: Vec<String>,
    /// Tail buffer of agent message/thought text.
    pub tail_text: String,
    /// Tail buffer of agent message text only.
    pub message_text: String,
    /// Tail buffer of agent thought text only.
    pub thought_text: String,
    /// Whether the output used thought text as fallback.
    pub has_thought_fallback: bool,
    /// Total input tokens reported by the underlying API response, when available.
    pub input_tokens: Option<u64>,
    /// Total output tokens reported by the underlying API response, when available.
    pub output_tokens: Option<u64>,
    /// Cache-read input tokens reported by the Anthropic API response.
    ///
    /// Anthropic returns `cache_read_input_tokens` in the response `usage`
    /// block when prompt caching is active. Older API responses and non-Claude
    /// backends may omit it, hence `Option`.
    pub cache_read_input_tokens: Option<u64>,
}

impl StreamingMetadata {
    /// Ratio of cache-read input tokens to total input tokens (`cache_read / input_tokens`).
    ///
    /// Returns `None` when either field is missing or when `input_tokens` is
    /// zero (no meaningful denominator).
    pub fn cache_hit_ratio(&self) -> Option<f64> {
        let cache_read = self.cache_read_input_tokens? as f64;
        let total_input = self.input_tokens? as f64;
        if total_input == 0.0 {
            return None;
        }
        Some(cache_read / total_input)
    }
}

/// Streaming session events collected from agent notifications.
#[derive(Debug, Clone, serde::Serialize)]
pub enum SessionEvent {
    AgentMessage(String),
    AgentThought(String),
    ToolCallStarted {
        id: String,
        title: String,
        kind: String,
    },
    ToolCallCompleted {
        id: String,
        status: String,
    },
    PlanUpdate(String),
    Other(String),
}
