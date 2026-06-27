use anyhow::Result;

use csa_config::GlobalConfig;
use csa_core::types::ToolName;

/// Check if a prompt is a context compress/compact command.
pub(crate) fn is_compress_command(prompt: &str) -> bool {
    let trimmed = prompt.trim();
    trimmed == "/compress" || trimmed == "/compact" || trimmed.starts_with("/compact ")
}

/// Parse a tool name string to ToolName enum.
pub(crate) fn parse_tool_name(name: &str) -> Result<ToolName> {
    match name {
        "gemini-cli" | "gemini" => anyhow::bail!("{}", csa_core::types::removed_tool_error(name)),
        "opencode" => Ok(ToolName::Opencode),
        "codex" => Ok(ToolName::Codex),
        "claude-code" => Ok(ToolName::ClaudeCode),
        "openai-compat" => Ok(ToolName::OpenaiCompat),
        "antigravity-cli" => Ok(ToolName::AntigravityCli),
        _ => anyhow::bail!("Unknown tool: {name}"),
    }
}

/// Truncate a string to max_len characters, adding "..." if truncated.
///
/// Uses character (not byte) counting to safely handle multi-byte UTF-8.
pub(crate) fn truncate_prompt(s: &str, max_len: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_len {
        s.to_string()
    } else {
        // Find byte offset for the character at position (max_len - 3)
        let truncate_at_chars = max_len.saturating_sub(3);
        let byte_offset = s
            .char_indices()
            .nth(truncate_at_chars)
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        let substring = &s[..byte_offset];

        // Try to break at last space if possible
        if let Some(last_space) = substring.rfind(' ')
            && last_space > byte_offset / 2
        {
            return format!("{}...", &s[..last_space]);
        }

        format!("{substring}...")
    }
}

/// Detect the parent tool context.
///
/// Resolution order:
/// 1. `CSA_TOOL` environment variable (set by CSA when spawning children)
/// 2. `CSA_PARENT_TOOL` environment variable (set for grandchild processes)
/// 3. Process tree walking via `/proc` (Linux-only fallback)
pub(crate) fn detect_parent_tool() -> Option<String> {
    std::env::var("CSA_TOOL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var("CSA_PARENT_TOOL")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
        .or_else(crate::process_tree::detect_ancestor_tool)
}

/// Resolve parent tool context using detection result with global-config fallback.
///
/// Resolution order:
/// 1. Detected parent tool from runtime context
/// 2. `~/.config/cli-sub-agent/config.toml` `[defaults].tool`
/// 3. None
///
/// The literal `"auto"` is the documented auto-select sentinel (see
/// `csa-config::tool_selection`), NOT a concrete tool name. It is normalized to
/// `None` here so callers that feed the result into `parse_tool_name` (the
/// heterogeneous strategy arms) treat "no concrete parent" rather than bailing
/// with `Unknown tool: auto`. Without this, a pinned SA-nested worker spawned in
/// a detached process tree (no ancestor tool detectable) and a global
/// `[defaults].tool = "auto"` would crash instead of honoring the inherited
/// `--model-spec` pin (#1741).
pub(crate) fn resolve_tool(detected: Option<String>, config: &GlobalConfig) -> Option<String> {
    detected
        .or_else(|| config.defaults.tool.clone())
        .filter(|tool| tool.trim() != "auto" && !tool.trim().is_empty())
}
