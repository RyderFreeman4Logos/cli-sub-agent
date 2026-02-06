//! Model specification parsing.

use anyhow::{bail, Result};
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
    /// Accepts: low, medium/med, high, xhigh/extra-high, or a numeric value.
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "low" => Ok(Self::Low),
            "medium" | "med" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" | "extra-high" => Ok(Self::Xhigh),
            other => {
                if let Ok(n) = other.parse::<u32>() {
                    Ok(Self::Custom(n))
                } else {
                    bail!(
                        "Invalid thinking budget '{}': expected low/medium/high/xhigh or a number",
                        other
                    )
                }
            }
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
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("expected tool/provider/model/thinking_budget"));
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
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid thinking budget"));
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
}
