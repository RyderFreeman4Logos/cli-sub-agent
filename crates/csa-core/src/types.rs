use clap::ValueEnum;
use serde::{Deserialize, Serialize};

/// AI tool selection
#[derive(Clone, Debug, ValueEnum, Serialize, Deserialize)]
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
}

/// Output format for CLI responses
#[derive(Clone, Debug, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}
