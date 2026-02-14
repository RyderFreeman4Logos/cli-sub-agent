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
    #[error("ACP process exited unexpectedly: code {0}")]
    ProcessExited(i32),
    #[error("ACP subprocess spawn failed: {0}")]
    SpawnFailed(#[from] std::io::Error),
    #[error("Configuration error: {0}")]
    ConfigError(String),
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
}
