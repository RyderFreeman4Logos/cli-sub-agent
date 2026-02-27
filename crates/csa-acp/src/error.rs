use thiserror::Error;

#[derive(Error, Debug)]
pub enum AcpError {
    #[error("ACP connection failed: {0}")]
    ConnectionFailed(String),
    #[error("ACP initialization failed: {0}")]
    InitializationFailed(String),
    #[error("ACP session creation failed: {0}")]
    SessionFailed(String),
    #[error("ACP prompt failed: {0}")]
    PromptFailed(String),
    #[error(
        "ACP process exited unexpectedly: code {code}{}",
        format_stderr(stderr)
    )]
    ProcessExited { code: i32, stderr: String },
    #[error("Session fork failed: {0}")]
    ForkFailed(String),
    #[error("ACP subprocess spawn failed: {0}")]
    SpawnFailed(#[from] std::io::Error),
    #[error("Configuration error: {0}")]
    ConfigError(String),
}

/// Format captured stderr for inclusion in `ProcessExited` error display.
///
/// Returns last 10 lines prefixed with `"; stderr:\n..."` or empty if no stderr.
fn format_stderr(stderr: &str) -> String {
    let trimmed = stderr.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        let last_lines: String = trimmed
            .lines()
            .rev()
            .take(10)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        format!("; stderr:\n{last_lines}")
    }
}

pub type AcpResult<T> = std::result::Result<T, AcpError>;

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::io;

    use super::AcpError;

    #[test]
    fn test_spawn_failed_display_and_source_chain() {
        let io_error = io::Error::new(io::ErrorKind::NotFound, "binary not found");
        let err = AcpError::from(io_error);

        assert_eq!(
            err.to_string(),
            "ACP subprocess spawn failed: binary not found"
        );
        let source = err.source().expect("spawn error should have source");
        assert_eq!(source.to_string(), "binary not found");
    }

    #[test]
    fn test_prompt_failed_display_without_source() {
        let err = AcpError::PromptFailed("permission denied".to_string());
        assert_eq!(err.to_string(), "ACP prompt failed: permission denied");
        assert!(err.source().is_none());
    }

    #[test]
    fn test_process_exited_without_stderr() {
        let err = AcpError::ProcessExited {
            code: 143,
            stderr: String::new(),
        };
        assert_eq!(err.to_string(), "ACP process exited unexpectedly: code 143");
    }

    #[test]
    fn test_process_exited_with_stderr() {
        let err = AcpError::ProcessExited {
            code: 1,
            stderr: "Error: write EPIPE\n  at node:events:486".to_string(),
        };
        let display = err.to_string();
        assert!(display.contains("code 1"), "should contain exit code");
        assert!(display.contains("EPIPE"), "should contain stderr content");
    }
}
