use super::*;

/// Configuration for the code review workflow.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum GateMode {
    /// Log review results only; never block execution.
    #[default]
    Monitor,
    /// Block only on Critical and High severity findings.
    CriticalOnly,
    /// Block on Critical, High, and Medium severity findings.
    Full,
}

/// A single step in the pre-review quality gate pipeline (L1–L3).
/// Steps execute sequentially in ascending level order; aborts on first failure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GateStep {
    pub name: String,
    pub command: String,
    /// Verification level: 1 = lint, 2 = type/boundary, 3 = test.
    #[serde(default = "default_gate_level")]
    pub level: u8,
}

const fn default_gate_level() -> u8 {
    1
}

/// Configuration for the code review workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewConfig {
    /// Review tool selection.
    ///
    /// Accepts a single tool name (`"codex"`), `"auto"` for heterogeneous
    /// auto-selection, or an array of tool names (`["codex", "claude-code"]`)
    /// as a whitelist for auto-selection. An empty array is equivalent to `"auto"`.
    #[serde(default)]
    pub tool: ToolSelection,
    /// Review enforcement level for quality gates.
    #[serde(default)]
    pub gate_mode: GateMode,
    /// Run cumulative review at most once per N commits.
    ///
    /// `1` preserves the existing behavior: review every round.
    /// Values `>= 2` allow intermediate workflows to skip cumulative review
    /// until at least N new commits have landed since the last passed
    /// `main...HEAD` cumulative review on the current branch.
    #[serde(
        default = "default_review_batch_commits",
        skip_serializing_if = "is_default_review_batch_commits"
    )]
    pub batch_commits: u32,
    /// Tier-based tool selection. When set, the review tool is resolved from the
    /// named tier's models list with heterogeneous preference. Takes priority
    /// over `tool` when both are set. The tier must exist in `[tiers]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    /// Default model for `csa review`. Overrides the tool's own default model
    /// selection without requiring a tier.
    /// Without an active tier: CLI `--model` > this field > tool default.
    /// With an active review tier, the tier model spec remains authoritative unless CLI `--model` is provided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Default thinking budget for `csa review` (`low`, `medium`, `high`, `xhigh`, `max`,
    /// or a token count).
    /// `csa review --thinking <LEVEL>` (when supported) overrides this.
    /// With an active review tier, the tier thinking budget remains authoritative unless CLI `--thinking` is provided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// Warn when the net review diff exceeds this many changed lines.
    ///
    /// `0` disables the warning. A missing field inherits the effective default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub large_diff_warn_lines: Option<usize>,
    /// Deprecated: prefer `gate_commands`. PROJECT-ONLY.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_command: Option<String>,
    /// Multi-layer gate pipeline (L1→L2→L3). Takes priority over `gate_command`. PROJECT-ONLY.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gate_commands: Vec<GateStep>,
    /// Timeout (seconds) for the pre-review quality gate command.
    ///
    /// PROJECT-ONLY: values set in global config are ignored during merge.
    #[serde(
        default = "default_gate_timeout_secs",
        skip_serializing_if = "is_default_gate_timeout"
    )]
    pub gate_timeout_secs: u64,
    /// Standard review is read-only by default; `csa review --fix` stays writable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readonly_sandbox: Option<bool>,
}

const fn default_gate_timeout_secs() -> u64 {
    250
}

const fn default_review_batch_commits() -> u32 {
    1
}

const DEFAULT_LARGE_DIFF_WARN_LINES: usize = 1000;

fn is_default_gate_timeout(val: &u64) -> bool {
    *val == default_gate_timeout_secs()
}

fn is_default_review_batch_commits(val: &u32) -> bool {
    *val == default_review_batch_commits()
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            tool: ToolSelection::default(),
            gate_mode: GateMode::default(),
            batch_commits: default_review_batch_commits(),
            tier: None,
            model: None,
            thinking: None,
            large_diff_warn_lines: Self::default_large_diff_warn_lines(),
            gate_command: None,
            gate_commands: Vec::new(),
            gate_timeout_secs: default_gate_timeout_secs(),
            readonly_sandbox: None,
        }
    }
}

impl ReviewConfig {
    /// Returns true when all fields match defaults (per rust/016 serde-default rule).
    pub fn is_default(&self) -> bool {
        self.tool.is_auto()
            && self.gate_mode == GateMode::Monitor
            && self.batch_commits == default_review_batch_commits()
            && self.tier.is_none()
            && self.model.is_none()
            && self.thinking.is_none()
            && self
                .large_diff_warn_lines
                .is_none_or(|value| value == DEFAULT_LARGE_DIFF_WARN_LINES)
            && self.gate_command.is_none()
            && self.gate_commands.is_empty()
            && self.gate_timeout_secs == default_gate_timeout_secs()
            && self.readonly_sandbox.is_none()
    }

    /// Returns the effective gate steps, preferring `gate_commands` over legacy
    /// `gate_command`. If both are empty, returns an empty vec.
    /// Always appends the L4 token-budget gate if the script exists.
    pub fn effective_gate_steps(&self) -> Vec<GateStep> {
        let mut steps = if !self.gate_commands.is_empty() {
            self.gate_commands.clone()
        } else if let Some(cmd) = &self.gate_command {
            vec![GateStep {
                name: "legacy-gate".to_string(),
                command: cmd.clone(),
                level: 1,
            }]
        } else {
            Vec::new()
        };

        let token_script = std::path::Path::new("scripts/hooks/token-budget-gate.sh");
        if token_script.exists() && !steps.iter().any(|s| s.name == "token-budget") {
            steps.push(GateStep {
                name: "token-budget".to_string(),
                command: token_script.display().to_string(),
                level: 4,
            });
        }

        steps.sort_by_key(|s| s.level);
        steps
    }

    /// Default gate timeout in seconds.
    pub const fn default_gate_timeout() -> u64 {
        default_gate_timeout_secs()
    }

    /// Default cumulative review batch size.
    pub const fn default_batch_commits() -> u32 {
        default_review_batch_commits()
    }

    pub const fn default_large_diff_warn_lines() -> Option<usize> {
        Some(DEFAULT_LARGE_DIFF_WARN_LINES)
    }
}

/// Configuration for the debate workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebateConfig {
    /// Debate tool selection.
    ///
    /// Accepts a single tool name (`"codex"`), `"auto"` for heterogeneous
    /// auto-selection, or an array of tool names (`["codex", "claude-code"]`)
    /// as a whitelist for auto-selection. An empty array is equivalent to `"auto"`.
    #[serde(default)]
    pub tool: ToolSelection,
    /// Default absolute wall-clock timeout (seconds) for `csa debate`.
    ///
    /// `csa debate --timeout <N>` overrides this per invocation.
    #[serde(default = "default_debate_timeout_seconds")]
    pub timeout_seconds: u64,
    /// Default model for `csa debate`. Overrides the tool's own default model
    /// selection without requiring a tier.
    /// Without an active tier: CLI `--model` > this field > tool default.
    /// With an active debate tier, the tier model spec remains authoritative unless CLI `--model` is provided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Default thinking budget for `csa debate` (`low`, `medium`, `high`, `xhigh`, `max`,
    /// or a token count).
    /// `csa debate --thinking <LEVEL>` overrides this per invocation.
    /// With an active debate tier, the tier thinking budget remains authoritative unless CLI `--thinking` is provided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// Allow same-model adversarial fallback when heterogeneous models are unavailable.
    ///
    /// When enabled (default), `csa debate` falls back to running two independent
    /// sub-agents of the same tool as Proposer and Critic. The debate output is
    /// annotated with "same-model adversarial" to indicate degraded diversity.
    ///
    /// Set to `false` to require heterogeneous models (strict mode).
    #[serde(default = "default_true_debate")]
    pub same_model_fallback: bool,
    /// Fail fast when a multi-tool debate tier collapses to a single surviving tool.
    ///
    /// Default: false. When true, tier resolution errors instead of silently
    /// proceeding with a narrowed single-tool panel.
    #[serde(default)]
    pub require_heterogeneous: bool,
    /// Tier-based tool selection. When set, the debate tool is resolved from the
    /// named tier's models list with heterogeneous preference. Takes priority
    /// over `tool` when both are set. The tier must exist in `[tiers]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    /// When true, enforce filesystem-level read-only access to the project root
    /// during debate sessions. This prevents the debate tool from writing files
    /// even if instructed to. Default: false (allows resume-to-fix workflow).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readonly_sandbox: Option<bool>,
}

fn default_debate_timeout_seconds() -> u64 {
    1800
}

fn default_true_debate() -> bool {
    true
}

impl Default for DebateConfig {
    fn default() -> Self {
        Self {
            tool: ToolSelection::default(),
            timeout_seconds: default_debate_timeout_seconds(),
            model: None,
            thinking: None,
            same_model_fallback: true,
            require_heterogeneous: false,
            tier: None,
            readonly_sandbox: None,
        }
    }
}

impl DebateConfig {
    /// Returns true when all fields match defaults (per rust/016 serde-default rule).
    pub fn is_default(&self) -> bool {
        self.tool.is_auto()
            && self.timeout_seconds == default_debate_timeout_seconds()
            && self.model.is_none()
            && self.thinking.is_none()
            && self.same_model_fallback
            && !self.require_heterogeneous
            && self.tier.is_none()
            && self.readonly_sandbox.is_none()
    }
}

/// Configuration for fallback behavior when external services are unavailable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackConfig {
    /// Behavior when cloud review bot is unavailable (quota, timeout, or API errors).
    ///
    /// - `"auto-local"`: Automatically fall back to local CSA review (still reviews)
    /// - `"ask-user"`: Prompt user before falling back (default)
    ///
    /// Both policies ensure code is still reviewed — `auto-local` just skips the
    /// user confirmation prompt. There is no `skip` option because bypassing
    /// review entirely violates the heterogeneous review safety model.
    #[serde(default = "default_cloud_review_exhausted")]
    pub cloud_review_exhausted: String,
}

fn default_cloud_review_exhausted() -> String {
    "ask-user".to_string()
}

impl Default for FallbackConfig {
    fn default() -> Self {
        Self {
            cloud_review_exhausted: default_cloud_review_exhausted(),
        }
    }
}
