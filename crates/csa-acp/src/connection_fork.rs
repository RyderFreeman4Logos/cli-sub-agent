use std::path::Path;
use std::time::Duration;

use crate::error::{AcpError, AcpResult};

/// Result of a successful CLI-based session fork.
#[derive(Debug, Clone)]
pub struct CliForkResult {
    /// The new provider-level session ID created by the fork.
    pub session_id: String,
}

/// Fork a Claude Code session via the `claude` CLI.
///
/// Spawns `claude --resume <provider_session_id> --fork-session -p "." --output-format json`
/// and parses the JSON output to extract the new session ID.
///
/// This is a blocking CLI operation (not ACP protocol) used to create a
/// provider-level fork before attaching via `load_session()`.
pub async fn fork_session_via_cli(
    provider_session_id: &str,
    working_dir: &Path,
    timeout: Duration,
) -> AcpResult<CliForkResult> {
    use tokio::process::Command;

    let mut cmd = Command::new("claude");
    cmd.arg("--resume")
        .arg(provider_session_id)
        .arg("--fork-session")
        .arg("-p")
        .arg(".")
        .arg("--output-format")
        .arg("json")
        .current_dir(working_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    // Strip parent Claude Code env vars to avoid recursion detection.
    for var in super::AcpConnection::STRIPPED_ENV_VARS {
        cmd.env_remove(var);
    }

    let mut child = cmd.spawn().map_err(|e| {
        AcpError::ForkFailed(format!("failed to spawn `claude --fork-session`: {e}"))
    })?;

    // Take stdout/stderr handles before waiting so we retain ownership of
    // `child` for cleanup if the timeout fires.  `wait_with_output()` consumes
    // `self`, which would prevent killing the process on timeout.
    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();

    // Read stdout/stderr concurrently with wait to avoid pipe-buffer deadlock.
    let wait_and_collect = async {
        let stdout_task = async {
            let mut buf = Vec::new();
            if let Some(mut h) = stdout_handle {
                tokio::io::AsyncReadExt::read_to_end(&mut h, &mut buf).await?;
            }
            Ok::<_, std::io::Error>(buf)
        };
        let stderr_task = async {
            let mut buf = Vec::new();
            if let Some(mut h) = stderr_handle {
                tokio::io::AsyncReadExt::read_to_end(&mut h, &mut buf).await?;
            }
            Ok::<_, std::io::Error>(buf)
        };
        let (status, stdout_buf, stderr_buf) =
            tokio::try_join!(child.wait(), stdout_task, stderr_task)?;
        Ok::<_, std::io::Error>(std::process::Output {
            status,
            stdout: stdout_buf,
            stderr: stderr_buf,
        })
    };

    let output = match tokio::time::timeout(timeout, wait_and_collect).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            return Err(AcpError::ForkFailed(format!(
                "claude --fork-session I/O error: {e}"
            )));
        }
        Err(_) => {
            // Kill the child process to prevent leaked background processes.
            // The child may have already exited between the timeout and this
            // kill attempt; that is harmless (kill returns Ok or a benign error).
            let _ = child.kill().await;
            return Err(AcpError::ForkFailed(format!(
                "claude --fork-session timed out after {}s; child process killed",
                timeout.as_secs()
            )));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let code = output.status.code().unwrap_or(-1);
        return Err(AcpError::ForkFailed(format!(
            "claude --fork-session exited with code {code}: {stderr}"
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_fork_json_output(&stdout)
}

/// Parse the JSON output from `claude --fork-session --output-format json`.
///
/// Expected format (at minimum): `{"session_id": "..."}` or `{"sessionId": "..."}`.
/// The parser tries multiple field names for forward compatibility.
pub(crate) fn parse_fork_json_output(json_str: &str) -> AcpResult<CliForkResult> {
    let trimmed = json_str.trim();
    if trimmed.is_empty() {
        return Err(AcpError::ForkFailed(
            "claude --fork-session produced empty output".to_string(),
        ));
    }

    let value: serde_json::Value = serde_json::from_str(trimmed).map_err(|e| {
        AcpError::ForkFailed(format!(
            "failed to parse claude --fork-session JSON output: {e}; raw: {trimmed}"
        ))
    })?;

    // Try multiple field names for resilience against API changes.
    let session_id = extract_session_id_from_json(&value).ok_or_else(|| {
        AcpError::ForkFailed(format!(
            "claude --fork-session JSON missing session ID field; got: {trimmed}"
        ))
    })?;

    if session_id.is_empty() {
        return Err(AcpError::ForkFailed(
            "claude --fork-session returned empty session ID".to_string(),
        ));
    }

    Ok(CliForkResult { session_id })
}

/// Extract session ID from JSON, trying multiple field name conventions.
fn extract_session_id_from_json(value: &serde_json::Value) -> Option<String> {
    // Primary: snake_case (session_id)
    if let Some(id) = value.get("session_id").and_then(|v| v.as_str()) {
        return Some(id.to_string());
    }
    // Alternative: camelCase (sessionId) — common in JS-origin tools
    if let Some(id) = value.get("sessionId").and_then(|v| v.as_str()) {
        return Some(id.to_string());
    }
    // Alternative: nested under "result" or "data"
    for wrapper in ["result", "data"] {
        if let Some(inner) = value.get(wrapper) {
            if let Some(id) = inner.get("session_id").and_then(|v| v.as_str()) {
                return Some(id.to_string());
            }
            if let Some(id) = inner.get("sessionId").and_then(|v| v.as_str()) {
                return Some(id.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_fork_json_output ---

    #[test]
    fn test_parse_fork_json_snake_case_session_id() {
        let json = r#"{"session_id": "abc-123-def"}"#;
        let result = parse_fork_json_output(json).expect("should parse");
        assert_eq!(result.session_id, "abc-123-def");
    }

    #[test]
    fn test_parse_fork_json_camel_case_session_id() {
        let json = r#"{"sessionId": "camel-456"}"#;
        let result = parse_fork_json_output(json).expect("should parse");
        assert_eq!(result.session_id, "camel-456");
    }

    #[test]
    fn test_parse_fork_json_nested_under_result() {
        let json = r#"{"result": {"session_id": "nested-789"}}"#;
        let result = parse_fork_json_output(json).expect("should parse");
        assert_eq!(result.session_id, "nested-789");
    }

    #[test]
    fn test_parse_fork_json_nested_under_data_camel_case() {
        let json = r#"{"data": {"sessionId": "data-camel-101"}}"#;
        let result = parse_fork_json_output(json).expect("should parse");
        assert_eq!(result.session_id, "data-camel-101");
    }

    #[test]
    fn test_parse_fork_json_with_extra_fields() {
        let json = r#"{"session_id": "extra-fields", "status": "ok", "version": 2}"#;
        let result = parse_fork_json_output(json).expect("should parse");
        assert_eq!(result.session_id, "extra-fields");
    }

    #[test]
    fn test_parse_fork_json_with_whitespace() {
        let json = "  \n  {\"session_id\": \"whitespace-ok\"}  \n  ";
        let result = parse_fork_json_output(json).expect("should parse");
        assert_eq!(result.session_id, "whitespace-ok");
    }

    #[test]
    fn test_parse_fork_json_empty_input() {
        let err = parse_fork_json_output("").unwrap_err();
        assert!(err.to_string().contains("empty output"), "got: {err}");
    }

    #[test]
    fn test_parse_fork_json_whitespace_only() {
        let err = parse_fork_json_output("   \n  ").unwrap_err();
        assert!(err.to_string().contains("empty output"), "got: {err}");
    }

    #[test]
    fn test_parse_fork_json_invalid_json() {
        let err = parse_fork_json_output("not json at all").unwrap_err();
        assert!(err.to_string().contains("failed to parse"), "got: {err}");
    }

    #[test]
    fn test_parse_fork_json_missing_session_id_field() {
        let json = r#"{"status": "ok", "message": "forked"}"#;
        let err = parse_fork_json_output(json).unwrap_err();
        assert!(err.to_string().contains("missing session ID"), "got: {err}");
    }

    #[test]
    fn test_parse_fork_json_empty_session_id() {
        let json = r#"{"session_id": ""}"#;
        let err = parse_fork_json_output(json).unwrap_err();
        assert!(err.to_string().contains("empty session ID"), "got: {err}");
    }

    #[test]
    fn test_parse_fork_json_session_id_not_string() {
        let json = r#"{"session_id": 12345}"#;
        let err = parse_fork_json_output(json).unwrap_err();
        assert!(err.to_string().contains("missing session ID"), "got: {err}");
    }

    #[test]
    fn test_parse_fork_json_snake_case_takes_priority() {
        // When both field names exist, snake_case wins.
        let json = r#"{"session_id": "snake-wins", "sessionId": "camel-loses"}"#;
        let result = parse_fork_json_output(json).expect("should parse");
        assert_eq!(result.session_id, "snake-wins");
    }

    #[test]
    fn test_parse_fork_json_top_level_beats_nested() {
        // Top-level field takes priority over nested.
        let json = r#"{"session_id": "top-level", "result": {"session_id": "nested"}}"#;
        let result = parse_fork_json_output(json).expect("should parse");
        assert_eq!(result.session_id, "top-level");
    }

    // --- extract_session_id_from_json ---

    #[test]
    fn test_extract_returns_none_for_empty_object() {
        let value: serde_json::Value = serde_json::json!({});
        assert!(extract_session_id_from_json(&value).is_none());
    }

    #[test]
    fn test_extract_returns_none_for_null_value() {
        let value: serde_json::Value = serde_json::json!({"session_id": null});
        assert!(extract_session_id_from_json(&value).is_none());
    }

    #[test]
    fn test_extract_returns_none_for_array_value() {
        let value: serde_json::Value = serde_json::json!({"session_id": ["a", "b"]});
        assert!(extract_session_id_from_json(&value).is_none());
    }
}
