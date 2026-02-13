use clap::ValueEnum;
use serde::{Deserialize, Serialize};

/// AI tool selection
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
pub enum ToolName {
    GeminiCli,
    Opencode,
    Codex,
    ClaudeCode,
}

impl ToolName {
    /// Returns the CLI-facing name for this tool
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::GeminiCli => "gemini-cli",
            Self::Opencode => "opencode",
            Self::Codex => "codex",
            Self::ClaudeCode => "claude-code",
        }
    }

    /// Returns the model family for this tool
    pub fn model_family(&self) -> ModelFamily {
        match self {
            Self::ClaudeCode => ModelFamily::Claude,
            Self::GeminiCli => ModelFamily::Gemini,
            Self::Codex => ModelFamily::OpenAI,
            Self::Opencode => ModelFamily::Other,
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
        ToolName::GeminiCli => PROMPT_TRANSPORT_ARGV_AND_STDIN,
        ToolName::ClaudeCode => PROMPT_TRANSPORT_ARGV_AND_STDIN,
        ToolName::Opencode => PROMPT_TRANSPORT_ARGV_ONLY,
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

/// CLI-level tool argument parsed from `--tool`.
#[derive(Clone, Debug)]
pub enum ToolArg {
    /// Auto-select (HeterogeneousStrict). Default when --tool omitted.
    Auto,
    /// First available tool in built-in preference order, no heterogeneity requirement.
    AnyAvailable,
    /// Explicit tool, no negotiation.
    Specific(ToolName),
}

impl std::str::FromStr for ToolArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(Self::Auto),
            "any-available" => Ok(Self::AnyAvailable),
            other => {
                // Try to parse as ToolName using clap's ValueEnum
                // We need to iterate through all possible values
                match other {
                    "gemini-cli" => Ok(Self::Specific(ToolName::GeminiCli)),
                    "opencode" => Ok(Self::Specific(ToolName::Opencode)),
                    "codex" => Ok(Self::Specific(ToolName::Codex)),
                    "claude-code" => Ok(Self::Specific(ToolName::ClaudeCode)),
                    _ => Err(format!(
                        "Invalid tool argument '{}'. Valid values: auto, any-available, gemini-cli, opencode, codex, claude-code",
                        other
                    )),
                }
            }
        }
    }
}

impl ToolArg {
    /// Convert to execution strategy based on command context.
    pub fn into_strategy(self) -> ToolSelectionStrategy {
        match self {
            Self::Auto => ToolSelectionStrategy::HeterogeneousStrict,
            Self::AnyAvailable => ToolSelectionStrategy::AnyAvailable,
            Self::Specific(t) => ToolSelectionStrategy::Explicit(t),
        }
    }
}

impl std::fmt::Display for ToolArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auto => write!(f, "auto"),
            Self::AnyAvailable => write!(f, "any-available"),
            Self::Specific(t) => write!(f, "{}", t.as_str()),
        }
    }
}

/// Resolved tool selection strategy used by the execution pipeline.
#[derive(Clone, Debug)]
pub enum ToolSelectionStrategy {
    /// Must use a different model family than the parent. Fails with reverse prompt if impossible.
    HeterogeneousStrict,
    /// Any available tool, no heterogeneity constraint.
    AnyAvailable,
    /// Explicitly specified tool.
    Explicit(ToolName),
}

/// Output format for CLI responses
#[derive(Clone, Debug, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_tool_arg_from_str_auto() {
        let arg = ToolArg::from_str("auto").unwrap();
        assert!(matches!(arg, ToolArg::Auto));
    }

    #[test]
    fn test_tool_arg_from_str_any_available() {
        let arg = ToolArg::from_str("any-available").unwrap();
        assert!(matches!(arg, ToolArg::AnyAvailable));
    }

    #[test]
    fn test_tool_arg_from_str_specific_gemini() {
        let arg = ToolArg::from_str("gemini-cli").unwrap();
        match arg {
            ToolArg::Specific(ToolName::GeminiCli) => {}
            _ => panic!("Expected Specific(GeminiCli)"),
        }
    }

    #[test]
    fn test_tool_arg_from_str_specific_codex() {
        let arg = ToolArg::from_str("codex").unwrap();
        match arg {
            ToolArg::Specific(ToolName::Codex) => {}
            _ => panic!("Expected Specific(Codex)"),
        }
    }

    #[test]
    fn test_tool_arg_from_str_invalid() {
        let result = ToolArg::from_str("invalid-tool");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("Invalid tool argument 'invalid-tool'")
        );
    }

    #[test]
    fn test_tool_arg_into_strategy_auto() {
        let strategy = ToolArg::Auto.into_strategy();
        assert!(matches!(
            strategy,
            ToolSelectionStrategy::HeterogeneousStrict
        ));
    }

    #[test]
    fn test_tool_arg_into_strategy_any_available() {
        let strategy = ToolArg::AnyAvailable.into_strategy();
        assert!(matches!(strategy, ToolSelectionStrategy::AnyAvailable));
    }

    #[test]
    fn test_tool_arg_into_strategy_specific() {
        let strategy = ToolArg::Specific(ToolName::Codex).into_strategy();
        match strategy {
            ToolSelectionStrategy::Explicit(ToolName::Codex) => {}
            _ => panic!("Expected Explicit(Codex)"),
        }
    }

    #[test]
    fn test_tool_name_model_family() {
        assert_eq!(ToolName::ClaudeCode.model_family(), ModelFamily::Claude);
        assert_eq!(ToolName::GeminiCli.model_family(), ModelFamily::Gemini);
        assert_eq!(ToolName::Codex.model_family(), ModelFamily::OpenAI);
        assert_eq!(ToolName::Opencode.model_family(), ModelFamily::Other);
    }

    #[test]
    fn test_tool_arg_display() {
        assert_eq!(ToolArg::Auto.to_string(), "auto");
        assert_eq!(ToolArg::AnyAvailable.to_string(), "any-available");
        assert_eq!(
            ToolArg::Specific(ToolName::GeminiCli).to_string(),
            "gemini-cli"
        );
    }

    #[test]
    fn test_model_family_display() {
        assert_eq!(ModelFamily::Claude.to_string(), "Claude");
        assert_eq!(ModelFamily::Gemini.to_string(), "Gemini");
        assert_eq!(ModelFamily::OpenAI.to_string(), "OpenAI");
        assert_eq!(ModelFamily::Other.to_string(), "Other");
    }

    #[test]
    fn test_tool_name_as_str() {
        assert_eq!(ToolName::GeminiCli.as_str(), "gemini-cli");
        assert_eq!(ToolName::Opencode.as_str(), "opencode");
        assert_eq!(ToolName::Codex.as_str(), "codex");
        assert_eq!(ToolName::ClaudeCode.as_str(), "claude-code");
    }

    #[test]
    fn test_tool_name_display() {
        assert_eq!(ToolName::GeminiCli.to_string(), "gemini-cli");
        assert_eq!(ToolName::Opencode.to_string(), "opencode");
        assert_eq!(ToolName::Codex.to_string(), "codex");
        assert_eq!(ToolName::ClaudeCode.to_string(), "claude-code");
    }

    #[test]
    fn test_tool_arg_from_str_specific_opencode() {
        let arg = ToolArg::from_str("opencode").unwrap();
        assert!(matches!(arg, ToolArg::Specific(ToolName::Opencode)));
    }

    #[test]
    fn test_tool_arg_from_str_specific_claude_code() {
        let arg = ToolArg::from_str("claude-code").unwrap();
        assert!(matches!(arg, ToolArg::Specific(ToolName::ClaudeCode)));
    }

    #[test]
    fn test_tool_arg_display_fromstr_roundtrip() {
        let cases = [
            ToolArg::Auto,
            ToolArg::AnyAvailable,
            ToolArg::Specific(ToolName::GeminiCli),
            ToolArg::Specific(ToolName::Opencode),
            ToolArg::Specific(ToolName::Codex),
            ToolArg::Specific(ToolName::ClaudeCode),
        ];
        for original in &cases {
            let s = original.to_string();
            let parsed = ToolArg::from_str(&s).unwrap();
            assert_eq!(parsed.to_string(), s);
        }
    }

    #[test]
    fn test_tool_arg_into_strategy_all_specific() {
        let tools = [
            (ToolName::GeminiCli, "GeminiCli"),
            (ToolName::Opencode, "Opencode"),
            (ToolName::Codex, "Codex"),
            (ToolName::ClaudeCode, "ClaudeCode"),
        ];
        for (tool, label) in tools {
            let strategy = ToolArg::Specific(tool).into_strategy();
            match strategy {
                ToolSelectionStrategy::Explicit(t) => assert_eq!(t, tool, "Mismatch for {label}"),
                _ => panic!("Expected Explicit for {label}"),
            }
        }
    }

    #[test]
    fn test_prompt_transport_capabilities() {
        assert_eq!(
            prompt_transport_capabilities(&ToolName::GeminiCli),
            &[PromptTransport::Argv, PromptTransport::Stdin]
        );
        assert_eq!(
            prompt_transport_capabilities(&ToolName::Codex),
            &[PromptTransport::Argv, PromptTransport::Stdin]
        );
        assert_eq!(
            prompt_transport_capabilities(&ToolName::ClaudeCode),
            &[PromptTransport::Argv, PromptTransport::Stdin]
        );
        assert_eq!(
            prompt_transport_capabilities(&ToolName::Opencode),
            &[PromptTransport::Argv]
        );
    }

    #[test]
    fn test_tool_arg_from_str_empty_string() {
        let result = ToolArg::from_str("");
        assert!(result.is_err());
    }

    #[test]
    fn test_tool_arg_from_str_case_sensitive() {
        // Tool names are case-sensitive: "Auto" should fail
        assert!(ToolArg::from_str("Auto").is_err());
        assert!(ToolArg::from_str("CODEX").is_err());
        assert!(ToolArg::from_str("Claude-Code").is_err());
    }
}
