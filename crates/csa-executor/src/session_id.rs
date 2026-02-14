//! Session ID extraction from tool output.
//!
//! Attempts to parse provider-native session IDs from tool stdout.
//! Returns None on extraction failure (graceful degradation).

use csa_core::types::ToolName;
use regex::Regex;
use tracing::debug;

use crate::transport::TransportResult;

/// Attempt to extract the provider's native session ID from tool output.
///
/// Returns None if extraction fails (graceful degradation).
pub fn extract_session_id(tool: &ToolName, output: &str) -> Option<String> {
    match tool {
        ToolName::GeminiCli => extract_gemini_session_id(output),
        ToolName::Opencode => extract_opencode_session_id(output),
        ToolName::Codex => extract_codex_session_id(output),
        ToolName::ClaudeCode => extract_claude_session_id(output),
    }
}

/// Extract provider session ID from transport result.
///
/// ACP transport provides a direct provider session ID. Legacy transport does
/// not, so this falls back to parsing tool stdout.
pub fn extract_session_id_from_transport(
    tool: &ToolName,
    transport_result: &TransportResult,
) -> Option<String> {
    if let Some(session_id) = &transport_result.provider_session_id {
        debug!("Using provider session ID from transport metadata");
        return Some(session_id.clone());
    }

    extract_session_id(tool, &transport_result.execution.output)
}

/// Extract Codex session ID from JSON output.
///
/// Codex exec returns JSON with "session_id" or "thread_id" field.
/// Example: {"session_id":"thread_abc123", ...}
fn extract_codex_session_id(output: &str) -> Option<String> {
    // Try simple string search first (faster than regex for simple patterns)
    if let Some(session_id) = extract_json_field(output, "session_id") {
        debug!("Extracted Codex session_id: {}", session_id);
        return Some(session_id);
    }

    if let Some(thread_id) = extract_json_field(output, "thread_id") {
        debug!("Extracted Codex thread_id: {}", thread_id);
        return Some(thread_id);
    }

    debug!("Failed to extract Codex session ID from output");
    None
}

/// Extract ClaudeCode session ID from JSON output.
///
/// ClaudeCode with --output-format json returns "session_id" field.
fn extract_claude_session_id(output: &str) -> Option<String> {
    if let Some(session_id) = extract_json_field(output, "session_id") {
        debug!("Extracted ClaudeCode session_id: {}", session_id);
        return Some(session_id);
    }

    debug!("Failed to extract ClaudeCode session ID from output");
    None
}

/// Extract Gemini CLI session ID.
///
/// Gemini CLI may not output session IDs in text mode.
/// Returns None for now (graceful degradation).
fn extract_gemini_session_id(_output: &str) -> Option<String> {
    debug!("Gemini CLI session ID extraction not implemented (no known pattern)");
    None
}

/// Extract Opencode session ID from JSON output.
///
/// Opencode with --format json may include session-related fields.
fn extract_opencode_session_id(output: &str) -> Option<String> {
    if let Some(session_id) = extract_json_field(output, "session_id") {
        debug!("Extracted Opencode session_id: {}", session_id);
        return Some(session_id);
    }

    if let Some(session_id) = extract_json_field(output, "sessionId") {
        debug!("Extracted Opencode sessionId: {}", session_id);
        return Some(session_id);
    }

    debug!("Failed to extract Opencode session ID from output");
    None
}

/// Extract a JSON field value using simple string matching.
///
/// Looks for pattern: "field_name":"value" or "field_name": "value"
/// Does NOT handle all JSON edge cases, but sufficient for session IDs.
fn extract_json_field(output: &str, field_name: &str) -> Option<String> {
    // Pattern: "field_name":"value" or "field_name": "value"
    // Capture group extracts the value (excluding quotes)
    let pattern_str = format!(r#""{}"\s*:\s*"([^"]+)""#, regex::escape(field_name));

    let re = Regex::new(&pattern_str).ok()?;
    let captures = re.captures(output)?;

    captures.get(1).map(|m| m.as_str().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_codex_session_id() {
        let output = r#"{"session_id":"thread_abc123","status":"success"}"#;
        let result = extract_codex_session_id(output);
        assert_eq!(result, Some("thread_abc123".to_string()));
    }

    #[test]
    fn test_extract_codex_thread_id() {
        let output = r#"{"thread_id":"thread_xyz789","status":"success"}"#;
        let result = extract_codex_session_id(output);
        assert_eq!(result, Some("thread_xyz789".to_string()));
    }

    #[test]
    fn test_extract_codex_with_spaces() {
        let output = r#"{ "session_id" : "thread_with_spaces" , "status": "ok" }"#;
        let result = extract_codex_session_id(output);
        assert_eq!(result, Some("thread_with_spaces".to_string()));
    }

    #[test]
    fn test_extract_claude_session_id() {
        let output = r#"{"session_id":"claude_session_456","model":"opus"}"#;
        let result = extract_claude_session_id(output);
        assert_eq!(result, Some("claude_session_456".to_string()));
    }

    #[test]
    fn test_extract_opencode_session_id() {
        let output = r#"{"session_id":"opencode_123","result":"done"}"#;
        let result = extract_opencode_session_id(output);
        assert_eq!(result, Some("opencode_123".to_string()));
    }

    #[test]
    fn test_extract_opencode_camel_case() {
        let output = r#"{"sessionId":"opencode_camel_456","result":"done"}"#;
        let result = extract_opencode_session_id(output);
        assert_eq!(result, Some("opencode_camel_456".to_string()));
    }

    #[test]
    fn test_extract_gemini_returns_none() {
        let output = "Some gemini output without session ID";
        let result = extract_gemini_session_id(output);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_empty_output() {
        assert_eq!(extract_codex_session_id(""), None);
        assert_eq!(extract_claude_session_id(""), None);
        assert_eq!(extract_opencode_session_id(""), None);
    }

    #[test]
    fn test_extract_malformed_json() {
        let output = r#"{"session_id":"incomplete"#;
        assert_eq!(extract_codex_session_id(output), None);
    }

    #[test]
    fn test_extract_no_session_field() {
        let output = r#"{"status":"success","result":"done"}"#;
        assert_eq!(extract_codex_session_id(output), None);
        assert_eq!(extract_claude_session_id(output), None);
    }

    #[test]
    fn test_extract_session_id_from_tool_name() {
        let codex_output = r#"{"session_id":"thread_123","status":"ok"}"#;
        let result = extract_session_id(&ToolName::Codex, codex_output);
        assert_eq!(result, Some("thread_123".to_string()));

        let claude_output = r#"{"session_id":"claude_456","model":"opus"}"#;
        let result = extract_session_id(&ToolName::ClaudeCode, claude_output);
        assert_eq!(result, Some("claude_456".to_string()));

        let gemini_output = "gemini output";
        let result = extract_session_id(&ToolName::GeminiCli, gemini_output);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_json_field() {
        let json = r#"{"key":"value","number":123}"#;
        assert_eq!(extract_json_field(json, "key"), Some("value".to_string()));

        let json_with_spaces = r#"{ "key" : "value" , "other": "field" }"#;
        assert_eq!(
            extract_json_field(json_with_spaces, "key"),
            Some("value".to_string())
        );

        assert_eq!(extract_json_field(json, "nonexistent"), None);
    }

    #[test]
    fn test_extract_session_id_from_transport_prefers_provider_session_id() {
        let transport_result = TransportResult {
            execution: csa_process::ExecutionResult {
                output: r#"{"session_id":"thread_from_output"}"#.to_string(),
                stderr_output: String::new(),
                summary: "ok".to_string(),
                exit_code: 0,
            },
            provider_session_id: Some("thread_from_transport".to_string()),
            events: Vec::new(),
        };

        let result = extract_session_id_from_transport(&ToolName::Codex, &transport_result);
        assert_eq!(result, Some("thread_from_transport".to_string()));
    }

    #[test]
    fn test_extract_session_id_from_transport_falls_back_to_output_parse() {
        let transport_result = TransportResult {
            execution: csa_process::ExecutionResult {
                output: r#"{"session_id":"thread_from_output"}"#.to_string(),
                stderr_output: String::new(),
                summary: "ok".to_string(),
                exit_code: 0,
            },
            provider_session_id: None,
            events: Vec::new(),
        };

        let result = extract_session_id_from_transport(&ToolName::Codex, &transport_result);
        assert_eq!(result, Some("thread_from_output".to_string()));
    }
}
