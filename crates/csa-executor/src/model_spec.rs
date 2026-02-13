//! Model specification parsing.

use anyhow::{Result, bail};
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

/// Thinking budget for AI models.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ThinkingBudget {
    /// Use the tool's default thinking budget.
    DefaultBudget,
    Low,
    Medium,
    High,
    Xhigh,
    Custom(u32),
}

impl ModelSpec {
    /// Parse model spec from string format: tool/provider/model/thinking_budget
    pub fn parse(spec: &str) -> Result<Self> {
        let parts: Vec<&str> = spec.splitn(4, '/').collect();
        if parts.len() != 4 {
            bail!(
                "Invalid model spec '{}': expected tool/provider/model/thinking_budget",
                spec
            );
        }
        Ok(Self {
            tool: parts[0].to_string(),
            provider: parts[1].to_string(),
            model: parts[2].to_string(),
            thinking_budget: ThinkingBudget::parse(parts[3])?,
        })
    }
}

impl ThinkingBudget {
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
            other => {
                if let Ok(n) = other.parse::<u32>() {
                    Ok(Self::Custom(n))
                } else {
                    bail!(
                        "Invalid thinking budget '{}': expected default/low/medium/high/xhigh or a number",
                        other
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
            Self::Custom(n) => *n,
        }
    }

    /// Returns the reasoning effort level for codex-style tools.
    ///
    /// Maps thinking budget levels to codex's --reasoning-effort values.
    pub fn codex_effort(&self) -> &'static str {
        match self {
            Self::DefaultBudget => "medium",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "high", // codex doesn't support xhigh, fallback to high
            Self::Custom(_) => "high", // custom values map to high
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
        assert_eq!(ThinkingBudget::Xhigh.codex_effort(), "high"); // fallback to high
        assert_eq!(ThinkingBudget::Custom(10000).codex_effort(), "high"); // fallback to high
    }
}
