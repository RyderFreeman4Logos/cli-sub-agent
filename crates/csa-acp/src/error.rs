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
