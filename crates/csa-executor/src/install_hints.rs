//! Shared install hints for executor-facing tool availability messages.

pub const GEMINI_CLI_INSTALL_HINT: &str = "Install: npm install -g @google/gemini-cli";
pub const OPENCODE_INSTALL_HINT: &str = "Install: go install github.com/sst/opencode@latest";
pub const CLAUDE_CODE_ACP_INSTALL_HINT: &str =
    "Install ACP adapter: npm install -g @zed-industries/claude-code-acp";
pub const OPENAI_COMPAT_INSTALL_HINT: &str =
    "Configure [tools.openai-compat] with base_url and api_key in config.toml";

pub fn install_hint_for_known_tool(tool_name: &str) -> Option<&'static str> {
    match tool_name {
        "gemini-cli" => Some(GEMINI_CLI_INSTALL_HINT),
        "opencode" => Some(OPENCODE_INSTALL_HINT),
        "claude-code" => Some(CLAUDE_CODE_ACP_INSTALL_HINT),
        "openai-compat" => Some(OPENAI_COMPAT_INSTALL_HINT),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_tool_install_hints_match_current_upstreams() {
        assert_eq!(
            install_hint_for_known_tool("gemini-cli"),
            Some(GEMINI_CLI_INSTALL_HINT)
        );
        assert_eq!(
            install_hint_for_known_tool("opencode"),
            Some(OPENCODE_INSTALL_HINT)
        );
        assert_eq!(
            install_hint_for_known_tool("claude-code"),
            Some(CLAUDE_CODE_ACP_INSTALL_HINT)
        );
        assert_eq!(
            install_hint_for_known_tool("openai-compat"),
            Some(OPENAI_COMPAT_INSTALL_HINT)
        );
        assert_eq!(install_hint_for_known_tool("codex"), None);
    }
}
