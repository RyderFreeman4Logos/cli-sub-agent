//! Session state types

use crate::output_section::ReturnPacketRef;
use chrono::{DateTime, Utc};
use csa_core::types::ReviewDecision;
use csa_core::vcs::{VcsIdentity, VcsKind};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};

fn default_identity_version() -> u8 {
    1
}

fn default_review_iterations() -> u32 {
    1
}

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

    /// CSA binary version that created this persisted session state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csa_version: Option<String>,

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

    /// Git porcelain snapshot at session creation time.
    /// Used to subtract pre-existing dirty tracked files from repo-write audit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_session_porcelain: Option<String>,

    /// Reference to the latest child return packet captured via fork-call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_return_packet: Option<ReturnPacketRef>,

    /// VCS change identifier bound to this session (e.g., jj change-id or git commit hash).
    /// Enables session-change binding so sessions can be grouped by logical change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_id: Option<String>,

    /// Spec document ULID or path associated with this session.
    /// Links the session to a specific agent-spec contract for traceability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_id: Option<String>,

    /// Unified VCS identity snapshot at session creation.
    /// When present, supersedes the legacy `branch`, `git_head_at_creation`,
    /// and `change_id` fields. Use `resolved_identity()` to get the effective identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vcs_identity: Option<VcsIdentity>,

    /// Identity schema version: 1 = legacy (separate fields), 2 = unified VcsIdentity.
    /// Used to determine trust level for identity comparisons.
    #[serde(default = "default_identity_version")]
    pub identity_version: u8,

    /// In-memory fork-call timestamps for simple per-session rate limiting.
    ///
    /// This is intentionally runtime-only and is not persisted to state.toml.
    #[serde(skip)]
    pub fork_call_timestamps: Vec<Instant>,
}

impl Default for MetaSessionState {
    fn default() -> Self {
        Self {
            meta_session_id: String::new(),
            description: None,
            project_path: String::new(),
            branch: None,
            created_at: DateTime::<Utc>::default(),
            last_accessed: DateTime::<Utc>::default(),
            csa_version: None,
            genealogy: Genealogy::default(),
            tools: HashMap::new(),
            context_status: ContextStatus::default(),
            total_token_usage: None,
            phase: SessionPhase::default(),
            task_context: TaskContext::default(),
            turn_count: 0,
            token_budget: None,
            sandbox_info: None,
            termination_reason: None,
            is_seed_candidate: false,
            git_head_at_creation: None,
            pre_session_porcelain: None,
            last_return_packet: None,
            change_id: None,
            spec_id: None,
            vcs_identity: None,
            identity_version: default_identity_version(),
            fork_call_timestamps: Vec::new(),
        }
    }
}

/// Structured metadata about a review execution.
///
/// Written to `{session_dir}/review_meta.json` after `csa review` completes,
/// enabling machine-readable access to review results for downstream consumers
/// (e.g., pr-bot, commit skill, orchestration scripts).
///
/// Updated after each fix round when `--fix` is enabled.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewSessionMeta {
    /// CSA meta-session ID for this review.
    pub session_id: String,
    /// Git HEAD SHA at review time.
    pub head_sha: String,
    /// Five-value review decision: pass, fail, skip, uncertain, unavailable.
    pub decision: String,
    /// Legacy verdict string (CLEAN, HAS_ISSUES, etc.) for backward compatibility.
    pub verdict: String,
    /// Review mode that produced this verdict ("standard" or "red-team").
    ///
    /// Absent for legacy sessions written before review-mode auditing (#1817).
    /// The merge gate uses this to enforce that a final adversarial review ran
    /// when `--check-verdict` is invoked with `--red-team` / `--review-mode`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_mode: Option<String>,
    /// Optional machine-readable reason when the review result is not a real verdict
    /// (for example, an auth/setup failure that prevented the review from running).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_reason: Option<String>,
    /// Tier model spec actually used after fallback, when it differs from the primary choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routed_to: Option<String>,
    /// Classified failure reason for the primary tier entry, optionally chained with
    /// intermediate fallback failures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_failure: Option<String>,
    /// Human-readable explanation when the reviewer infrastructure could not run to completion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    /// Tool used for this review (e.g., "claude-code", "codex").
    pub tool: String,
    /// Review scope (e.g., "uncommitted", "range:main...HEAD", "base:main").
    pub scope: String,
    /// Exit code of the review process.
    pub exit_code: i32,
    /// Whether `--fix` was enabled and a fix pass was attempted.
    pub fix_attempted: bool,
    /// Number of fix rounds completed (0 if no fix attempted).
    pub fix_rounds: u32,
    /// Number of outer review cycles observed on the same branch/PR.
    #[serde(default = "default_review_iterations")]
    pub review_iterations: u32,
    /// ISO 8601 review timestamp used to order candidate verdicts.
    ///
    /// Recovered metadata preserves the original review/verdict time rather than
    /// the recovery write time so stale reviews cannot be reordered as newest.
    pub timestamp: DateTime<Utc>,
    /// Content hash of the diff being reviewed (e.g., "sha256:abc123...").
    /// Enables deduplication: revert-revert scenarios with identical diffs
    /// can reuse a previous review without re-running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_fingerprint: Option<String>,
    /// Positive sentinel for `csa review --fix` convergence.
    ///
    /// Absent for non-fix and legacy sessions.  For `fix_attempted=true`,
    /// absence means the fix session did not prove genuine clean convergence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix_convergence: Option<FixConvergenceMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FixConvergenceMeta {
    pub quality_gate_passed: bool,
    pub fix_output_was_substantive: bool,
    pub post_consistency_decision: String,
    pub reached_genuine_clean_convergence: bool,
    pub terminal_reason: String,
}

impl ReviewSessionMeta {
    /// Returns true when review metadata represents an incomplete or failed
    /// reviewer execution that must not be treated as a clean verdict.
    pub fn requires_fail_closed_verdict(&self) -> bool {
        match self.decision.parse::<ReviewDecision>() {
            Ok(ReviewDecision::Unavailable | ReviewDecision::Uncertain) | Err(_) => return true,
            Ok(ReviewDecision::Pass | ReviewDecision::Fail | ReviewDecision::Skip) => {}
        }

        if self.status_reason.is_some() || self.failure_reason.is_some() {
            return true;
        }

        let has_primary_failure = self
            .primary_failure
            .as_deref()
            .is_some_and(|failure| !failure.trim().is_empty());

        has_primary_failure && self.exit_code != 0
    }

    /// True when this review is not a fix session, or when the fix session
    /// persisted the positive clean-convergence sentinel.
    pub fn fix_clean_converged(&self) -> bool {
        !self.fix_attempted
            || self
                .fix_convergence
                .as_ref()
                .is_some_and(|fix| fix.reached_genuine_clean_convergence)
    }

    /// Central acceptance predicate for consumers that need a clean review gate.
    pub fn accepts_clean_review_verdict(&self, artifact_decision: ReviewDecision) -> bool {
        artifact_decision == ReviewDecision::Pass
            && self.exit_code == 0
            && !self.requires_fail_closed_verdict()
            && self.fix_clean_converged()
    }
}

/// Write review session metadata to the session directory.
///
/// Creates or overwrites `{session_dir}/review_meta.json`.
pub fn write_review_meta(
    session_dir: &std::path::Path,
    meta: &ReviewSessionMeta,
) -> std::io::Result<()> {
    let path = session_dir.join("review_meta.json");
    let json = serde_json::to_string_pretty(meta).map_err(std::io::Error::other)?;
    std::fs::write(path, json)
}

/// Lightweight telemetry about the resource sandbox applied to a session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SandboxInfo {
    /// Sandbox isolation mode used: "cgroup", "rlimit", or "none".
    pub mode: String,
    /// Memory limit applied (MB), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_max_mb: Option<u64>,
    /// Filesystem isolation mode used: "bwrap", "landlock", or "none".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filesystem_mode: Option<String>,
    /// Whether the project root was mounted read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readonly_project_root: Option<bool>,
    /// Provenance for inherited and final resource values used by this child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_resolution: Option<ResourceResolutionInfo>,
}

/// Source of a resource value resolved for one CSA child.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResourceValueSource {
    ExplicitCli,
    InheritedParentExplicit,
    Configuration,
    ToolDefault,
    DocumentedDefault,
}

/// One resource value together with its typed provenance.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourcedResourceValue {
    /// Resource value in megabytes.
    pub value: u64,
    /// Boundary that supplied the value.
    pub source: ResourceValueSource,
}

/// Resource inheritance and final resolution recorded in child state.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceResolutionInfo {
    /// Memory limit explicitly inherited from the parent plan, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherited_memory_max_mb: Option<SourcedResourceValue>,
    /// Final memory limit after CLI, inheritance, config, and default precedence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_memory_max_mb: Option<SourcedResourceValue>,
    /// Free-memory threshold explicitly inherited from the parent plan, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherited_min_free_memory_mb: Option<SourcedResourceValue>,
    /// Final free-memory threshold after precedence resolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_min_free_memory_mb: Option<SourcedResourceValue>,
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

    /// Best-effort detected tool binary version recorded at tool initialization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_version: Option<String>,

    /// Token usage for this tool in this session
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<TokenUsage>,
}

// Token usage/budget accounting lives in a sibling module to keep this file
// under the per-module token budget; re-exported so `state::{TokenUsage,
// TokenBudget}` and the crate facade paths stay stable.
#[path = "state_token_budget.rs"]
mod token_budget;
pub use token_budget::{TokenBudget, TokenUsage};

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
    /// Tool account quota is permanently exhausted for this run.
    ToolExhausted,
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
    /// Tool account quota is exhausted; caller action is required before reuse.
    ToolExhausted,
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
    ///   Active --ToolExhausted--> ToolExhausted
    /// ```
    ///
    /// All other combinations are invalid.
    pub fn transition(&self, event: &PhaseEvent) -> Result<SessionPhase, String> {
        match (self, event) {
            (SessionPhase::Active, PhaseEvent::Compressed) => Ok(SessionPhase::Available),
            (SessionPhase::Active, PhaseEvent::Retired) => Ok(SessionPhase::Retired),
            (SessionPhase::Active, PhaseEvent::ToolExhausted) => Ok(SessionPhase::ToolExhausted),
            (SessionPhase::Available, PhaseEvent::Resumed) => Ok(SessionPhase::Active),
            (SessionPhase::Available, PhaseEvent::Retired) => Ok(SessionPhase::Retired),
            (current, event) => Err(format!("invalid phase transition: {current:?} + {event:?}")),
        }
    }
}

impl MetaSessionState {
    /// Get the effective VCS identity for this session.
    ///
    /// If `vcs_identity` is present (v2), returns it directly.
    /// Otherwise, constructs a legacy identity from the separate fields.
    pub fn resolved_identity(&self) -> VcsIdentity {
        if let Some(ref id) = self.vcs_identity {
            return id.clone();
        }
        // Construct from legacy fields.
        // Detect jj: change_id present AND (git_head absent OR change_id != git_head)
        let is_jj = self.change_id.is_some()
            && (self.git_head_at_creation.is_none() || self.change_id != self.git_head_at_creation);
        VcsIdentity {
            vcs_kind: if is_jj { VcsKind::Jj } else { VcsKind::Git },
            commit_id: self.git_head_at_creation.clone(),
            change_id: if is_jj { self.change_id.clone() } else { None },
            short_id: None,
            ref_name: self.branch.clone(),
            op_id: None,
        }
    }

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
            SessionPhase::ToolExhausted => write!(f, "tool_exhausted"),
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
