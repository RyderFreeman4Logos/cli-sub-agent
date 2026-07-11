//! Model specification parsing.

use anyhow::{Result, bail};
use csa_core::{
    model_catalog::{CatalogAdmission, CatalogErrorKind, EffectiveModelCatalog},
    thinking_budget,
};
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
    #[error("malformed model identity: {detail}")]
    MalformedIdentity { detail: String },
    #[error("unknown tool '{got}': valid tools are {valid:?}")]
    UnknownTool { got: String, valid: Vec<String> },
    #[error("unknown provider '{got}' for tool '{tool}': valid providers are {valid:?}")]
    UnknownProvider {
        tool: String,
        got: String,
        valid: Vec<String>,
    },
    #[error(
        "unknown model '{got}' for tool '{tool}' provider '{provider}': valid models are {valid:?}; {detail}"
    )]
    UnknownModel {
        tool: String,
        provider: String,
        got: String,
        valid: Vec<String>,
        detail: String,
    },
    #[error(
        "model '{got}' for tool '{tool}' provider '{provider}' is disabled by catalog tombstone: {detail}"
    )]
    DisabledModel {
        tool: String,
        provider: String,
        got: String,
        detail: String,
    },
    #[error(
        "unsupported reasoning effort '{got}' for tool '{tool}' provider '{provider}' model '{model}': {detail}"
    )]
    UnsupportedReasoningEffort {
        tool: String,
        provider: String,
        model: String,
        got: String,
        detail: String,
    },
    #[error(
        "unsupported custom reasoning budget '{got}' for tool '{tool}' provider '{provider}' model '{model}': {detail}"
    )]
    UnsupportedCustomReasoning {
        tool: String,
        provider: String,
        model: String,
        got: String,
        detail: String,
    },
}

/// Thinking budget for AI models.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

    /// Parse and validate against an effective per-command catalog in one step.
    pub fn parse_and_validate(
        spec: &str,
        catalog: &EffectiveModelCatalog,
        valid_tools: &[&'static str],
    ) -> Result<(Self, CatalogAdmission)> {
        let parsed = Self::parse(spec)?;
        let admission = parsed
            .validate_with_catalog(catalog, valid_tools)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok((parsed, admission))
    }

    /// Validate parsed spec against the immutable effective per-command catalog.
    pub fn validate_with_catalog(
        &self,
        catalog: &EffectiveModelCatalog,
        valid_tools: &[&str],
    ) -> std::result::Result<CatalogAdmission, ModelSpecValidationError> {
        if !valid_tools.contains(&self.tool.as_str()) {
            return Err(ModelSpecValidationError::UnknownTool {
                got: self.tool.clone(),
                valid: valid_tools.iter().map(|tool| (*tool).to_string()).collect(),
            });
        }

        let reasoning = match &self.thinking_budget {
            ThinkingBudget::DefaultBudget => "default".to_string(),
            ThinkingBudget::Low => "low".to_string(),
            ThinkingBudget::Medium => "medium".to_string(),
            ThinkingBudget::High => "high".to_string(),
            ThinkingBudget::Xhigh => "xhigh".to_string(),
            ThinkingBudget::Max => "max".to_string(),
            ThinkingBudget::Custom(value) => value.to_string(),
        };
        catalog
            .validate_parts(&self.tool, &self.provider, &self.model, &reasoning)
            .map_err(|error| match error.kind() {
                CatalogErrorKind::MalformedIdentity => {
                    ModelSpecValidationError::MalformedIdentity {
                        detail: error.to_string(),
                    }
                }
                CatalogErrorKind::UnknownTool => ModelSpecValidationError::UnknownTool {
                    got: self.tool.clone(),
                    valid: error.known().to_vec(),
                },
                CatalogErrorKind::UnknownProvider => ModelSpecValidationError::UnknownProvider {
                    tool: self.tool.clone(),
                    got: self.provider.clone(),
                    valid: error.known().to_vec(),
                },
                CatalogErrorKind::UnknownModel => ModelSpecValidationError::UnknownModel {
                    tool: self.tool.clone(),
                    provider: self.provider.clone(),
                    got: self.model.clone(),
                    valid: error.known().to_vec(),
                    detail: error.to_string(),
                },
                CatalogErrorKind::DisabledModel => ModelSpecValidationError::DisabledModel {
                    tool: self.tool.clone(),
                    provider: self.provider.clone(),
                    got: self.model.clone(),
                    detail: error.to_string(),
                },
                CatalogErrorKind::UnsupportedReasoningEffort => {
                    ModelSpecValidationError::UnsupportedReasoningEffort {
                        tool: self.tool.clone(),
                        provider: self.provider.clone(),
                        model: self.model.clone(),
                        got: reasoning.clone(),
                        detail: error.to_string(),
                    }
                }
                CatalogErrorKind::UnsupportedCustomReasoning => {
                    ModelSpecValidationError::UnsupportedCustomReasoning {
                        tool: self.tool.clone(),
                        provider: self.provider.clone(),
                        model: self.model.clone(),
                        got: reasoning,
                        detail: error.to_string(),
                    }
                }
            })
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
    /// Accepts: default/none, low, medium/med, high, xhigh/extra-high, max, or a numeric value.
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "default" | "none" => Ok(Self::DefaultBudget),
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

    /// One-shot retry budget when a codex run stalls on the initial response.
    /// Transport retries must preserve the configured effort; any effort change
    /// belongs to the catalog-aware scheduler and a new session boundary.
    pub fn codex_stall_retry_budget(&self) -> Option<ThinkingBudget> {
        match self {
            Self::Xhigh | Self::Max => Some(self.clone()),
            _ => None,
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
#[path = "model_spec_tests.rs"]
mod tests;
