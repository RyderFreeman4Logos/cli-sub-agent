//! Model specification parsing.

use anyhow::{Result, bail};
use csa_core::{model_catalog, thinking_budget};
use serde::{Deserialize, Serialize};

/// Unified model spec: tool/provider/model/thinking_budget
///
/// Example: "opencode/google/gemini-2.5-pro/high"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSpec {
    pub tool: String,
    pub provider: String,
    pub model: String,
    pub thinking_budget: ThinkingBudget,
}

#[derive(Debug, thiserror::Error)]
pub enum ModelSpecValidationError {
    #[error("unknown tool '{got}': valid tools are {valid:?}")]
    UnknownTool {
        got: String,
        valid: Vec<&'static str>,
    },
    #[error("unknown provider '{got}' for tool '{tool}': valid providers are {valid:?}")]
    UnknownProvider {
        tool: String,
        got: String,
        valid: Vec<&'static str>,
    },
    #[error(
        "unknown model '{got}' for tool '{tool}' provider '{provider}': valid models are {valid:?}"
    )]
    UnknownModel {
        tool: String,
        provider: String,
        got: String,
        valid: Vec<&'static str>,
    },
}

/// Thinking budget for AI models.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ThinkingBudget {
    /// Use the tool's default thinking budget.
    DefaultBudget,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
    Custom(u32),
}

impl ModelSpec {
    /// Parse model spec from string format: tool/provider/model/thinking_budget
    pub fn parse(spec: &str) -> Result<Self> {
        let parts: Vec<&str> = spec.splitn(4, '/').collect();
        if parts.len() != 4 {
            bail!("Invalid model spec '{spec}': expected tool/provider/model/thinking_budget");
        }
        Ok(Self {
            tool: parts[0].to_string(),
            provider: parts[1].to_string(),
            model: parts[2].to_string(),
            thinking_budget: ThinkingBudget::parse(parts[3])?,
        })
    }

    /// Parse and validate against the per-tool catalog in one step.
    pub fn parse_and_validate(spec: &str, valid_tools: &[&'static str]) -> Result<Self> {
        let parsed = Self::parse(spec)?;
        parsed
            .validate_with_catalog(valid_tools)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(parsed)
    }

    /// Validate parsed spec against the offline catalog.
    ///
    /// Skips provider/model checks for tools (for example `openai-compat`) where
    /// the model space is user-defined.
    pub fn validate_with_catalog(
        &self,
        valid_tools: &[&'static str],
    ) -> std::result::Result<(), ModelSpecValidationError> {
        if !valid_tools.contains(&self.tool.as_str()) {
            return Err(ModelSpecValidationError::UnknownTool {
                got: self.tool.clone(),
                valid: valid_tools.to_vec(),
            });
        }
        if model_catalog::provider_validation_enabled(&self.tool) {
            let valid = model_catalog::valid_providers(&self.tool);
            if !valid.contains(&self.provider.as_str()) {
                return Err(ModelSpecValidationError::UnknownProvider {
                    tool: self.tool.clone(),
                    got: self.provider.clone(),
                    valid: valid.to_vec(),
                });
            }
        }
        if model_catalog::model_validation_enabled(&self.tool) {
            let valid = model_catalog::valid_models(&self.tool, &self.provider);
            if !valid.contains(&self.model.as_str()) {
                return Err(ModelSpecValidationError::UnknownModel {
                    tool: self.tool.clone(),
                    provider: self.provider.clone(),
                    got: self.model.clone(),
                    valid: valid.to_vec(),
                });
            }
        }
        Ok(())
    }
}

impl ThinkingBudget {
    /// Try to split a trailing `/thinking_budget` suffix from a model string.
    ///
    /// Returns `(model, Some(budget))` if the last `/`-separated segment is a valid
    /// thinking budget keyword (not numeric — avoids ambiguity with version numbers).
    /// Otherwise returns `(original, None)`.
    ///
    /// Examples:
    /// - `"google/gemini-3.1-pro-preview/xhigh"` → `("google/gemini-3.1-pro-preview", Some(Xhigh))`
    /// - `"gemini-3.1-pro-preview/high"` → `("gemini-3.1-pro-preview", Some(High))`
    /// - `"google/gemini-3.1-pro-preview"` → `("google/gemini-3.1-pro-preview", None)`
    /// - `"gemini-3.1-pro-preview"` → `("gemini-3.1-pro-preview", None)`
    pub fn try_split_from_model(model: &str) -> (&str, Option<Self>) {
        if let Some(pos) = model.rfind('/') {
            let suffix = &model[pos + 1..];
            // Delegate keyword validation to parse() so the valid-keyword set is defined
            // in exactly one place. Only match named keywords, not Custom(n) — numbers in
            // model names are common (e.g., "gpt-5.4") and would cause false positives.
            match Self::parse(suffix) {
                Ok(budget) if !matches!(budget, Self::Custom(_)) => (&model[..pos], Some(budget)),
                _ => (model, None),
            }
        } else {
            (model, None)
        }
    }

    /// Parse thinking budget from string.
    ///
    /// Accepts: default, low, medium/med, high, xhigh/extra-high, or a numeric value.
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "default" => Ok(Self::DefaultBudget),
            "low" => Ok(Self::Low),
            "medium" | "med" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" | "extra-high" => Ok(Self::Xhigh),
            "max" => Ok(Self::Max),
            other => {
                if let Ok(n) = other.parse::<u32>() {
                    Ok(Self::Custom(n))
                } else {
                    bail!(
                        "Invalid thinking budget '{other}': expected {}",
                        thinking_budget::VALID_BUDGET_DESCRIPTION
                    )
                }
            }
        }
    }

    /// Returns the token count for this thinking budget level.
    pub fn token_count(&self) -> u32 {
        match self {
            Self::DefaultBudget => 10000,
            Self::Low => 1024,
            Self::Medium => 8192,
            Self::High => 32768,
            Self::Xhigh => 65536,
            Self::Max => 131072,
            Self::Custom(n) => *n,
        }
    }

    /// One-shot downgrade target when a codex run at this budget stalls on the
    /// initial response. `None` means no retry — the budget is already low
    /// enough that stalling suggests a real failure, not an over-thinking stall.
    pub fn codex_stall_retry_downgrade(&self) -> Option<ThinkingBudget> {
        match self {
            Self::Xhigh | Self::Max => Some(ThinkingBudget::High),
            _ => None,
        }
    }

    /// One-level downshift target when an ACP idle disconnect is detected.
    ///
    /// Steps down: Max→High (skips Xhigh — on codex both map to "xhigh" effort,
    /// so Max→Xhigh would be a no-op), Xhigh→High, High→Medium, Medium→Low.
    /// Returns `None` for Low/DefaultBudget/Custom (already minimal; downshifting
    /// further would not help and the idle disconnect should propagate as-is).
    pub fn idle_disconnect_downshift(&self) -> Option<ThinkingBudget> {
        match self {
            // Skip Xhigh: codex_effort() maps both Max and Xhigh to "xhigh",
            // so Max→Xhigh is a no-op on codex (#1101).
            Self::Max => Some(ThinkingBudget::High),
            Self::Xhigh => Some(ThinkingBudget::High),
            Self::High => Some(ThinkingBudget::Medium),
            Self::Medium => Some(ThinkingBudget::Low),
            Self::Low | Self::DefaultBudget | Self::Custom(_) => None,
        }
    }

    /// Returns the reasoning effort level for codex-style tools.
    ///
    /// Maps thinking budget levels to codex's `-c model_reasoning_effort=` values.
    pub fn codex_effort(&self) -> &'static str {
        match self {
            Self::DefaultBudget => "medium",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "xhigh",      // codex has no 'max', map to its ceiling
            Self::Custom(_) => "high", // custom values map to high
        }
    }

    /// Returns the effort level keyword for claude-code 2.x's `--effort` flag.
    ///
    /// claude-code 2.1.119 accepts `--effort <level>` with `low/medium/high/
    /// xhigh/max` (no token-count form like the legacy `--thinking-budget`).
    /// `DefaultBudget` returns `None` so callers can omit the flag and let
    /// claude-code apply its built-in default. Numeric `Custom(n)` values have
    /// no direct level equivalent and map to `high` (mirroring `codex_effort`'s
    /// treatment for the same case).
    ///
    /// Replaces the pre-PR-#1120 `--thinking-budget <tokens>` emission, which
    /// claude-code 2.x rejects as `unknown option` (see issue #1124).
    pub fn claude_effort(&self) -> Option<&'static str> {
        match self {
            Self::DefaultBudget => None,
            Self::Low => Some("low"),
            Self::Medium => Some("medium"),
            Self::High => Some("high"),
            Self::Xhigh => Some("xhigh"),
            Self::Max => Some("max"),
            Self::Custom(_) => Some("high"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_spec() {
        let spec = ModelSpec::parse("opencode/google/gemini-2.5-pro/high").unwrap();
        assert_eq!(spec.tool, "opencode");
        assert_eq!(spec.provider, "google");
        assert_eq!(spec.model, "gemini-2.5-pro");
        assert!(matches!(spec.thinking_budget, ThinkingBudget::High));
    }

    #[test]
    fn test_parse_spec_with_custom_budget() {
        let spec = ModelSpec::parse("codex/anthropic/claude-opus/5000").unwrap();
        assert_eq!(spec.tool, "codex");
        assert_eq!(spec.provider, "anthropic");
        assert_eq!(spec.model, "claude-opus");
        assert!(matches!(spec.thinking_budget, ThinkingBudget::Custom(5000)));
    }

    #[test]
    fn test_parse_invalid_spec_wrong_parts() {
        let result = ModelSpec::parse("opencode/google/gemini");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected tool/provider/model/thinking_budget")
        );
    }

    #[test]
    fn validate_with_catalog_accepts_known_spec() {
        let spec = ModelSpec::parse("codex/openai/gpt-5.5/xhigh").unwrap();
        assert!(spec.validate_with_catalog(&["codex", "gemini-cli"]).is_ok());
    }

    #[test]
    fn rejects_opencode_cross_provider_gemini_under_openai() {
        let spec = ModelSpec::parse("opencode/openai/gemini-2.5-pro/high").unwrap();
        let err = spec.validate_with_catalog(&["opencode"]).unwrap_err();
        let message = err.to_string();

        assert!(message.contains("gemini-2.5-pro"));
        assert!(message.contains("openai"));
        assert!(message.contains("gpt-5"));
        assert!(!message.contains("claude-opus-4-7"));
    }

    #[test]
    fn rejects_opencode_cross_provider_claude_under_google() {
        let spec = ModelSpec::parse("opencode/google/claude-opus-4-7/high").unwrap();
        let err = spec.validate_with_catalog(&["opencode"]).unwrap_err();
        let message = err.to_string();

        assert!(message.contains("claude-opus-4-7"));
        assert!(message.contains("google"));
        assert!(message.contains("gemini-2.5-pro"));
        assert!(!message.contains("gpt-5"));
    }

    #[test]
    fn accepts_opencode_correct_pairing() {
        for raw in [
            "opencode/openai/gpt-5/high",
            "opencode/google/gemini-2.5-pro/high",
            "opencode/anthropic/claude-opus-4-7/high",
        ] {
            let spec = ModelSpec::parse(raw).unwrap();
            assert!(
                spec.validate_with_catalog(&["opencode"]).is_ok(),
                "{raw} should validate"
            );
        }
    }

    #[test]
    fn validate_with_catalog_rejects_unknown_tool() {
        let spec = ModelSpec::parse("unknown/openai/gpt-5.5/xhigh").unwrap();
        let err = spec.validate_with_catalog(&["codex"]).unwrap_err();
        assert!(err.to_string().contains("unknown"));
        assert!(err.to_string().contains("codex"));
    }

    #[test]
    fn validate_with_catalog_rejects_unknown_provider() {
        let spec = ModelSpec::parse("codex/anthropic/gpt-5.5/xhigh").unwrap();
        let err = spec.validate_with_catalog(&["codex"]).unwrap_err();
        assert!(err.to_string().contains("anthropic"));
        assert!(err.to_string().contains("openai"));
    }

    #[test]
    fn validate_with_catalog_rejects_unknown_model() {
        let spec = ModelSpec::parse("codex/openai/o3/xhigh").unwrap();
        let err = spec.validate_with_catalog(&["codex"]).unwrap_err();
        assert!(err.to_string().contains("o3"));
        assert!(err.to_string().contains("gpt-5.5"));
    }

    #[test]
    fn validate_with_catalog_skips_openai_compat_provider_and_model() {
        let spec = ModelSpec::parse("openai-compat/local/my-fine-tune/medium").unwrap();
        assert!(spec.validate_with_catalog(&["openai-compat"]).is_ok());
    }

    #[test]
    fn test_thinking_budget_parse_default() {
        assert!(matches!(
            ThinkingBudget::parse("default").unwrap(),
            ThinkingBudget::DefaultBudget
        ));
        assert!(matches!(
            ThinkingBudget::parse("Default").unwrap(),
            ThinkingBudget::DefaultBudget
        ));
        assert!(matches!(
            ThinkingBudget::parse("DEFAULT").unwrap(),
            ThinkingBudget::DefaultBudget
        ));
    }

    #[test]
    fn test_thinking_budget_parse_low() {
        let budget = ThinkingBudget::parse("low").unwrap();
        assert!(matches!(budget, ThinkingBudget::Low));
    }

    #[test]
    fn test_thinking_budget_parse_medium() {
        assert!(matches!(
            ThinkingBudget::parse("medium").unwrap(),
            ThinkingBudget::Medium
        ));
        assert!(matches!(
            ThinkingBudget::parse("med").unwrap(),
            ThinkingBudget::Medium
        ));
    }

    #[test]
    fn test_thinking_budget_parse_high() {
        let budget = ThinkingBudget::parse("high").unwrap();
        assert!(matches!(budget, ThinkingBudget::High));
    }

    #[test]
    fn test_thinking_budget_parse_xhigh() {
        assert!(matches!(
            ThinkingBudget::parse("xhigh").unwrap(),
            ThinkingBudget::Xhigh
        ));
        assert!(matches!(
            ThinkingBudget::parse("extra-high").unwrap(),
            ThinkingBudget::Xhigh
        ));
    }

    #[test]
    fn test_thinking_budget_parse_custom() {
        let budget = ThinkingBudget::parse("1234").unwrap();
        assert!(matches!(budget, ThinkingBudget::Custom(1234)));
    }

    #[test]
    fn test_thinking_budget_parse_invalid() {
        let result = ThinkingBudget::parse("invalid");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Invalid thinking budget")
        );
    }

    #[test]
    fn test_thinking_budget_case_insensitive() {
        assert!(matches!(
            ThinkingBudget::parse("LOW").unwrap(),
            ThinkingBudget::Low
        ));
        assert!(matches!(
            ThinkingBudget::parse("High").unwrap(),
            ThinkingBudget::High
        ));
        assert!(matches!(
            ThinkingBudget::parse("XHIGH").unwrap(),
            ThinkingBudget::Xhigh
        ));
    }

    #[test]
    fn test_thinking_budget_token_count() {
        assert_eq!(ThinkingBudget::DefaultBudget.token_count(), 10000);
        assert_eq!(ThinkingBudget::Low.token_count(), 1024);
        assert_eq!(ThinkingBudget::Medium.token_count(), 8192);
        assert_eq!(ThinkingBudget::High.token_count(), 32768);
        assert_eq!(ThinkingBudget::Xhigh.token_count(), 65536);
        assert_eq!(ThinkingBudget::Custom(5000).token_count(), 5000);
    }

    #[test]
    fn test_thinking_budget_codex_effort() {
        assert_eq!(ThinkingBudget::DefaultBudget.codex_effort(), "medium");
        assert_eq!(ThinkingBudget::Low.codex_effort(), "low");
        assert_eq!(ThinkingBudget::Medium.codex_effort(), "medium");
        assert_eq!(ThinkingBudget::High.codex_effort(), "high");
        assert_eq!(ThinkingBudget::Xhigh.codex_effort(), "xhigh");
        assert_eq!(ThinkingBudget::Custom(10000).codex_effort(), "high"); // fallback to high
    }

    #[test]
    fn test_thinking_budget_claude_effort() {
        // DefaultBudget = "let claude-code apply its own default" =>
        // omit the flag entirely (None).
        assert_eq!(ThinkingBudget::DefaultBudget.claude_effort(), None);
        assert_eq!(ThinkingBudget::Low.claude_effort(), Some("low"));
        assert_eq!(ThinkingBudget::Medium.claude_effort(), Some("medium"));
        assert_eq!(ThinkingBudget::High.claude_effort(), Some("high"));
        assert_eq!(ThinkingBudget::Xhigh.claude_effort(), Some("xhigh"));
        assert_eq!(ThinkingBudget::Max.claude_effort(), Some("max"));
        // Custom(n) has no level form in claude-code 2.x's --effort flag;
        // mirror codex_effort and pick "high" so the value stays accepted.
        assert_eq!(ThinkingBudget::Custom(10000).claude_effort(), Some("high"));
    }

    #[test]
    fn try_split_provider_model_thinking() {
        let (model, budget) =
            ThinkingBudget::try_split_from_model("google/gemini-3.1-pro-preview/xhigh");
        assert_eq!(model, "google/gemini-3.1-pro-preview");
        assert!(matches!(budget, Some(ThinkingBudget::Xhigh)));
    }

    #[test]
    fn try_split_model_thinking() {
        let (model, budget) = ThinkingBudget::try_split_from_model("gemini-3.1-pro-preview/high");
        assert_eq!(model, "gemini-3.1-pro-preview");
        assert!(matches!(budget, Some(ThinkingBudget::High)));
    }

    #[test]
    fn try_split_no_thinking_suffix() {
        let (model, budget) = ThinkingBudget::try_split_from_model("google/gemini-3.1-pro-preview");
        assert_eq!(model, "google/gemini-3.1-pro-preview");
        assert!(budget.is_none());
    }

    #[test]
    fn try_split_plain_model() {
        let (model, budget) = ThinkingBudget::try_split_from_model("gemini-3.1-pro-preview");
        assert_eq!(model, "gemini-3.1-pro-preview");
        assert!(budget.is_none());
    }

    #[test]
    fn try_split_numeric_suffix_not_split() {
        // Numeric suffixes should NOT be treated as thinking budgets —
        // too ambiguous with model version numbers.
        let (model, budget) = ThinkingBudget::try_split_from_model("gpt-5.4/1000");
        assert_eq!(model, "gpt-5.4/1000");
        assert!(budget.is_none());
    }

    #[test]
    fn try_split_case_insensitive() {
        let (model, budget) = ThinkingBudget::try_split_from_model("some-model/XHIGH");
        assert_eq!(model, "some-model");
        assert!(matches!(budget, Some(ThinkingBudget::Xhigh)));
    }

    #[test]
    fn test_thinking_budget_parse_max() {
        assert!(matches!(
            ThinkingBudget::parse("max").unwrap(),
            ThinkingBudget::Max
        ));
        assert!(matches!(
            ThinkingBudget::parse("MAX").unwrap(),
            ThinkingBudget::Max
        ));
    }

    #[test]
    fn test_thinking_budget_parse_error_mentions_max() {
        let err = ThinkingBudget::parse("invalid").unwrap_err().to_string();
        assert!(
            err.contains("max"),
            "error message should mention 'max': {err}"
        );
    }

    #[test]
    fn test_thinking_budget_max_token_count() {
        assert_eq!(ThinkingBudget::Max.token_count(), 131072);
    }

    #[test]
    fn test_thinking_budget_max_codex_effort() {
        assert_eq!(ThinkingBudget::Max.codex_effort(), "xhigh");
    }

    #[test]
    fn try_split_max_suffix() {
        let (model, budget) = ThinkingBudget::try_split_from_model("some-model/max");
        assert_eq!(model, "some-model");
        assert!(matches!(budget, Some(ThinkingBudget::Max)));
    }

    #[test]
    fn try_split_from_model_handles_max() {
        let (model, budget) = ThinkingBudget::try_split_from_model("gpt-5.4/max");
        assert_eq!(model, "gpt-5.4");
        assert!(matches!(budget, Some(ThinkingBudget::Max)));
    }

    #[test]
    fn codex_stall_retry_downgrade_covers_max() {
        assert!(matches!(
            ThinkingBudget::Max.codex_stall_retry_downgrade(),
            Some(ThinkingBudget::High)
        ));
        assert!(matches!(
            ThinkingBudget::Xhigh.codex_stall_retry_downgrade(),
            Some(ThinkingBudget::High)
        ));
        for budget in [
            ThinkingBudget::High,
            ThinkingBudget::Medium,
            ThinkingBudget::Low,
            ThinkingBudget::DefaultBudget,
            ThinkingBudget::Custom(50000),
        ] {
            assert!(
                budget.codex_stall_retry_downgrade().is_none(),
                "expected no downgrade for {budget:?}"
            );
        }
    }

    #[test]
    fn test_parse_spec_with_max_budget() {
        let spec = ModelSpec::parse("claude-code/anthropic/default/max").unwrap();
        assert_eq!(spec.tool, "claude-code");
        assert_eq!(spec.provider, "anthropic");
        assert_eq!(spec.model, "default");
        assert!(matches!(spec.thinking_budget, ThinkingBudget::Max));
    }
}
