#[derive(thiserror::Error, Debug)]
pub enum AppError {
    #[error("Session locked by PID {0}")]
    SessionLocked(u32),

    #[error("Invalid session ID '{0}': expected ULID format (26 chars Crockford Base32)")]
    InvalidSessionId(String),

    #[error("No session matching prefix '{0}'")]
    SessionNotFound(String),

    #[error("Ambiguous session prefix '{0}': matches multiple sessions")]
    AmbiguousSessionPrefix(String),

    #[error("Project root not found")]
    ProjectRootNotFound,

    #[error("Tool '{0}' is not installed")]
    ToolNotInstalled(String),

    #[error("Tool '{0}' is disabled for this project")]
    ToolDisabled(String),

    #[error("Tool execution failed: {0}")]
    ToolExecError(String),

    #[error("Max recursion depth exceeded (current: {current}, max: {max})")]
    MaxDepthExceeded { current: u32, max: u32 },

    #[error("Cannot operate on parent session from child")]
    ParentSessionViolation,

    #[error("Insufficient memory: available {available_mb} MB, need {required_mb} MB")]
    InsufficientMemory { available_mb: u64, required_mb: u64 },
}
