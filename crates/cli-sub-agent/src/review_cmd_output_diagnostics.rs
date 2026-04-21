use super::*;

/// Detect known tool-level diagnostic messages that indicate the review tool
/// failed to actually perform a review (e.g., gemini-cli MCP connectivity issues).
///
/// Checks both stdout and stderr for known failure patterns.
/// Returns a human-readable diagnostic summary when a known pattern is found.
pub(crate) fn detect_tool_diagnostic(stdout: &str, stderr: &str) -> Option<String> {
    let has_quota_issue = |text: &str| {
        let text_lower = text.to_ascii_lowercase();
        RATE_LIMIT_PATTERNS
            .iter()
            .copied()
            .any(|marker| text_lower.contains(marker))
            || text_lower.contains("quota will reset")
    };
    let has_mcp_issue =
        |text: &str| text.contains("MCP issues detected") || text.contains("Run /mcp list");

    if has_quota_issue(stdout) || has_quota_issue(stderr) {
        return Some(
            "gemini-cli OAuth quota exhausted. Either (a) configure GEMINI_API_KEY in ~/.config/cli-sub-agent/config.toml under [tools.gemini-cli] api_key for automatic retry, or (b) wait for quota reset."
                .to_string(),
        );
    }

    if has_mcp_issue(stdout) || has_mcp_issue(stderr) {
        return Some(
            "gemini-cli MCP init degraded. \
             Retry with `--force-ignore-tier-setting` + a different `--tool`, \
             or run `csa doctor` to diagnose unhealthy MCP servers."
                .to_string(),
        );
    }

    None
}
