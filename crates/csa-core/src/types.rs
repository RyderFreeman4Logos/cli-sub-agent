use std::collections::HashMap;

use chrono::{DateTime, Utc};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

/// AI tool selection
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
pub enum ToolName {
    #[value(skip)]
    GeminiCli,
    Opencode,
    Codex,
    ClaudeCode,
    OpenaiCompat,
    Hermes,
    AntigravityCli,
}

/// Primary CLI-backed tools surfaced by doctor and default tool listings.
pub const PRIMARY_TOOL_NAMES: &[&str] = &["opencode", "codex", "claude-code", "hermes"];

/// Tools eligible for general automatic routing and fallback.
///
/// `antigravity-cli` is intentionally omitted: it may still be invoked through
/// explicit low-risk paths, but CSA must not use it as a general fallback for
/// implementation, review, or long-context work.
pub const ROUTING_CANDIDATE_TOOLS: &[ToolName] = &[
    ToolName::Opencode,
    ToolName::Codex,
    ToolName::ClaudeCode,
    ToolName::OpenaiCompat,
    ToolName::Hermes,
];

pub fn is_removed_tool_name(name: &str) -> bool {
    matches!(name, "gemini-cli" | "gemini")
}

pub fn removed_tool_error(name: &str) -> String {
    format!(
        "tool '{name}' is no longer supported: gemini-cli integration has been removed because \
         the provider is discontinued. Remove it from CLI/config/tier mappings and use codex or \
         claude-code instead. CSA will not route to antigravity-cli as a general fallback."
    )
}

impl ToolName {
    /// Returns the CLI-facing name for this tool
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::GeminiCli => "gemini-cli",
            Self::Opencode => "opencode",
            Self::Codex => "codex",
            Self::ClaudeCode => "claude-code",
            Self::OpenaiCompat => "openai-compat",
            Self::Hermes => "hermes",
            Self::AntigravityCli => "antigravity-cli",
        }
    }

    /// Returns the model family for this tool
    pub fn model_family(&self) -> ModelFamily {
        match self {
            Self::ClaudeCode => ModelFamily::Claude,
            Self::GeminiCli | Self::AntigravityCli => ModelFamily::Gemini,
            Self::Codex => ModelFamily::OpenAI,
            Self::Opencode => ModelFamily::Other,
            Self::OpenaiCompat => ModelFamily::Other,
            Self::Hermes => ModelFamily::Other,
        }
    }

    /// Returns prompt transport channels supported by this tool.
    pub fn prompt_transport_capabilities(&self) -> &'static [PromptTransport] {
        prompt_transport_capabilities(self)
    }
}

/// Prompt transport channel used to send user prompts to tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromptTransport {
    Argv,
    Stdin,
}

const PROMPT_TRANSPORT_ARGV_ONLY: &[PromptTransport] = &[PromptTransport::Argv];
const PROMPT_TRANSPORT_ARGV_AND_STDIN: &[PromptTransport] =
    &[PromptTransport::Argv, PromptTransport::Stdin];

/// Prompt transport capabilities for each tool.
pub fn prompt_transport_capabilities(tool: &ToolName) -> &'static [PromptTransport] {
    match tool {
        ToolName::Codex => PROMPT_TRANSPORT_ARGV_AND_STDIN,
        ToolName::GeminiCli | ToolName::AntigravityCli => PROMPT_TRANSPORT_ARGV_AND_STDIN,
        ToolName::ClaudeCode => PROMPT_TRANSPORT_ARGV_AND_STDIN,
        ToolName::Hermes => PROMPT_TRANSPORT_ARGV_AND_STDIN,
        ToolName::Opencode => PROMPT_TRANSPORT_ARGV_ONLY,
        // OpenAI-compat is HTTP-only; prompt transport is irrelevant (no CLI process).
        // Return Stdin to satisfy callers that check capabilities.
        ToolName::OpenaiCompat => PROMPT_TRANSPORT_ARGV_AND_STDIN,
    }
}

impl std::fmt::Display for ToolName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Model family for heterogeneous diversity enforcement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModelFamily {
    Claude,
    Gemini,
    OpenAI,
    Other,
}

impl std::fmt::Display for ModelFamily {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Claude => write!(f, "Claude"),
            Self::Gemini => write!(f, "Gemini"),
            Self::OpenAI => write!(f, "OpenAI"),
            Self::Other => write!(f, "Other"),
        }
    }
}

/// Resolve the `ModelFamily` (quota-pool / provider grouping) for a CLI tool name.
///
/// Tools that share an upstream provider quota pool map to the same
/// `ModelFamily` so that failover can skip same-provider alternatives after one
/// of them exhausts the shared quota.
///
/// Legacy Gemini-family session records and `antigravity-cli` both consume the
/// Google/Gemini quota pool.
pub fn provider_for_tool_name(tool: &str) -> Option<ModelFamily> {
    match tool {
        "gemini-cli" | "antigravity-cli" | "antigravity" | "gemini" => Some(ModelFamily::Gemini),
        "claude-code" | "claude" => Some(ModelFamily::Claude),
        "codex" => Some(ModelFamily::OpenAI),
        "opencode" | "openai-compat" | "hermes" => Some(ModelFamily::Other),
        _ => None,
    }
}

/// One step in a quota/rate-limit failover chain: which tool/spec was tried and why it was skipped.
///
/// Written to `result.toml` under `[[fallback_chain]]` when failover occurred during `csa run`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackAttempt {
    /// Tool name that was attempted (e.g. "codex").
    pub tool: String,
    /// Full model spec that was attempted (e.g. "codex/openai/gpt-5.5/xhigh").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_spec: Option<String>,
    /// Machine-readable reason the tool was skipped (matched pattern from stderr/stdout).
    pub skip_reason: String,
    /// Whether this skip was due to permanent quota exhaustion (vs. transient rate limit).
    pub quota_exhausted: bool,
    /// UTC timestamp when the skip was recorded.
    pub timestamp: DateTime<Utc>,
}

/// CLI-level tool argument parsed from `--tool`.
#[derive(Clone, Debug)]
pub enum ToolArg {
    /// Auto-select (HeterogeneousPreferred). Default when --tool omitted.
    Auto,
    /// First available tool in built-in preference order, no heterogeneity requirement.
    AnyAvailable,
    /// Explicit tool, no negotiation.
    Specific(ToolName),
    /// Unresolved user alias — must be resolved via config `[tool_aliases]` before use.
    Alias(String),
}

impl std::str::FromStr for ToolArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(Self::Auto),
            "any-available" => Ok(Self::AnyAvailable),
            "gemini-cli" | "gemini" => Err(removed_tool_error(s)),
            // Canonical tool names
            "opencode" => Ok(Self::Specific(ToolName::Opencode)),
            "codex" => Ok(Self::Specific(ToolName::Codex)),
            "claude-code" => Ok(Self::Specific(ToolName::ClaudeCode)),
            "openai-compat" => Ok(Self::Specific(ToolName::OpenaiCompat)),
            "hermes" => Ok(Self::Specific(ToolName::Hermes)),
            "antigravity-cli" => Ok(Self::Specific(ToolName::AntigravityCli)),
            // Built-in aliases for common short names
            "claude" => Ok(Self::Specific(ToolName::ClaudeCode)),
            "antigravity" => Ok(Self::Specific(ToolName::AntigravityCli)),
            // Unknown string — store for config-based resolution
            other => Ok(Self::Alias(other.to_string())),
        }
    }
}

impl ToolArg {
    /// Resolve a config-based alias to a concrete `ToolArg`.
    ///
    /// Built-in aliases (`gemini`, `claude`) are already resolved in `from_str`.
    /// This method handles user-defined aliases from `[tool_aliases]` in config.
    /// Non-alias variants pass through unchanged.
    pub fn resolve_alias(self, tool_aliases: &HashMap<String, String>) -> Result<Self, String> {
        match self {
            Self::Alias(ref alias) => {
                if let Some(canonical) = tool_aliases.get(alias) {
                    // Recurse once to resolve the canonical name (which may itself
                    // be a built-in alias or canonical tool name).
                    let resolved: Self = canonical.parse()?;
                    match resolved {
                        Self::Alias(ref inner) => Err(format!(
                            "tool alias '{alias}' maps to '{inner}' which is not a valid tool \
                             name. Valid targets: opencode, codex, claude-code, openai-compat, hermes, antigravity-cli"
                        )),
                        other => Ok(other),
                    }
                } else {
                    Err(format!(
                        "unknown tool '{alias}'. Valid values: auto, any-available, \
                         opencode, codex, claude-code, openai-compat, hermes, antigravity-cli. \
                         Or define it in [tool_aliases] in config."
                    ))
                }
            }
            other => Ok(other),
        }
    }

    /// Convert to execution strategy based on command context.
    ///
    /// # Panics
    ///
    /// Panics if called on an unresolved `Alias` — callers must call
    /// `resolve_alias()` first.
    pub fn into_strategy(self) -> ToolSelectionStrategy {
        match self {
            Self::Auto => ToolSelectionStrategy::HeterogeneousPreferred,
            Self::AnyAvailable => ToolSelectionStrategy::AnyAvailable,
            Self::Specific(t) => ToolSelectionStrategy::Explicit(t),
            Self::Alias(a) => {
                panic!(
                    "BUG: unresolved tool alias '{a}' reached into_strategy(); resolve_alias() must be called first"
                )
            }
        }
    }
}

impl std::fmt::Display for ToolArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auto => write!(f, "auto"),
            Self::AnyAvailable => write!(f, "any-available"),
            Self::Specific(t) => write!(f, "{}", t.as_str()),
            Self::Alias(a) => write!(f, "{a}"),
        }
    }
}

/// Resolved tool selection strategy used by the execution pipeline.
#[derive(Clone, Debug)]
pub enum ToolSelectionStrategy {
    /// Must use a different model family than the parent. Fails with reverse prompt if impossible.
    HeterogeneousStrict,
    /// Try heterogeneous (different family), fall back to any available if impossible.
    HeterogeneousPreferred,
    /// Any available tool, no heterogeneity constraint.
    AnyAvailable,
    /// Explicitly specified tool.
    Explicit(ToolName),
}

/// Output format for CLI responses
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}

/// Five-value review decision semantics.
///
/// Replaces binary CLEAN/HAS_ISSUES with richer verdict vocabulary:
/// - `Pass`: All checks passed, no issues found.
/// - `Fail`: One or more blocking issues found.
/// - `Skip`: Review was skipped (e.g., gate not configured, depth guard).
/// - `Uncertain`: Reviewer could not reach a confident verdict.
/// - `Unavailable`: Reviewer infrastructure could not run to completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    Pass,
    Fail,
    Skip,
    Uncertain,
    #[serde(alias = "UNAVAILABLE")]
    Unavailable,
}

impl ReviewDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Skip => "skip",
            Self::Uncertain => "uncertain",
            Self::Unavailable => "unavailable",
        }
    }

    /// Whether this decision is considered "clean" (no blocking issues).
    pub fn is_clean(self) -> bool {
        matches!(self, Self::Pass | Self::Skip)
    }
}

impl std::fmt::Display for ReviewDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ReviewDecision {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "pass" | "clean" => Ok(Self::Pass),
            "fail" | "has_issues" => Ok(Self::Fail),
            "skip" | "skipped" => Ok(Self::Skip),
            "uncertain" | "unknown" => Ok(Self::Uncertain),
            "unavailable" => Ok(Self::Unavailable),
            _ => Err(format!(
                "Unknown review decision '{s}'. Expected: pass, fail, skip, uncertain, or unavailable."
            )),
        }
    }
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;
