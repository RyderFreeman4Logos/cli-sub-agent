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

    #[error("Rate limited by tool '{tool}': {message}")]
    RateLimited { tool: String, message: String },

    #[error("All tools in tier '{tier}' exhausted")]
    TierExhausted { tier: String },

    #[error("All {max} slots for '{tool}' are occupied")]
    SlotExhausted {
        tool: String,
        max: u32,
        /// (tool_name, free_slots, max_slots) for alternative tools.
        alternatives: Vec<(String, u32, u32)>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_session_locked() {
        let err = AppError::SessionLocked(1234);
        assert_eq!(err.to_string(), "Session locked by PID 1234");
    }

    #[test]
    fn test_display_invalid_session_id() {
        let err = AppError::InvalidSessionId("bad-id".into());
        assert_eq!(
            err.to_string(),
            "Invalid session ID 'bad-id': expected ULID format (26 chars Crockford Base32)"
        );
    }

    #[test]
    fn test_display_session_not_found() {
        let err = AppError::SessionNotFound("01ARZ".into());
        assert_eq!(err.to_string(), "No session matching prefix '01ARZ'");
    }

    #[test]
    fn test_display_ambiguous_session_prefix() {
        let err = AppError::AmbiguousSessionPrefix("01".into());
        assert_eq!(
            err.to_string(),
            "Ambiguous session prefix '01': matches multiple sessions"
        );
    }

    #[test]
    fn test_display_project_root_not_found() {
        let err = AppError::ProjectRootNotFound;
        assert_eq!(err.to_string(), "Project root not found");
    }

    #[test]
    fn test_display_tool_not_installed() {
        let err = AppError::ToolNotInstalled("gemini-cli".into());
        assert_eq!(err.to_string(), "Tool 'gemini-cli' is not installed");
    }

    #[test]
    fn test_display_tool_disabled() {
        let err = AppError::ToolDisabled("codex".into());
        assert_eq!(err.to_string(), "Tool 'codex' is disabled for this project");
    }

    #[test]
    fn test_display_tool_exec_error() {
        let err = AppError::ToolExecError("timeout after 30s".into());
        assert_eq!(err.to_string(), "Tool execution failed: timeout after 30s");
    }

    #[test]
    fn test_display_max_depth_exceeded() {
        let err = AppError::MaxDepthExceeded { current: 6, max: 5 };
        assert_eq!(
            err.to_string(),
            "Max recursion depth exceeded (current: 6, max: 5)"
        );
    }

    #[test]
    fn test_display_parent_session_violation() {
        let err = AppError::ParentSessionViolation;
        assert_eq!(
            err.to_string(),
            "Cannot operate on parent session from child"
        );
    }

    #[test]
    fn test_display_insufficient_memory() {
        let err = AppError::InsufficientMemory {
            available_mb: 256,
            required_mb: 512,
        };
        assert_eq!(
            err.to_string(),
            "Insufficient memory: available 256 MB, need 512 MB"
        );
    }

    #[test]
    fn test_display_rate_limited() {
        let err = AppError::RateLimited {
            tool: "gemini-cli".into(),
            message: "429 Too Many Requests".into(),
        };
        assert_eq!(
            err.to_string(),
            "Rate limited by tool 'gemini-cli': 429 Too Many Requests"
        );
    }

    #[test]
    fn test_display_tier_exhausted() {
        let err = AppError::TierExhausted {
            tier: "fast".into(),
        };
        assert_eq!(err.to_string(), "All tools in tier 'fast' exhausted");
    }

    #[test]
    fn test_display_slot_exhausted() {
        let err = AppError::SlotExhausted {
            tool: "codex".into(),
            max: 2,
            alternatives: vec![("opencode".into(), 1, 3)],
        };
        assert_eq!(err.to_string(), "All 2 slots for 'codex' are occupied");
    }

    #[test]
    fn test_error_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AppError>();
    }

    #[test]
    fn test_display_boundary_values() {
        // Zero PID
        let err = AppError::SessionLocked(0);
        assert_eq!(err.to_string(), "Session locked by PID 0");

        // Max depth values
        let err = AppError::MaxDepthExceeded {
            current: u32::MAX,
            max: u32::MAX,
        };
        assert!(err.to_string().contains(&u32::MAX.to_string()));

        // Empty strings
        let err = AppError::InvalidSessionId(String::new());
        assert_eq!(
            err.to_string(),
            "Invalid session ID '': expected ULID format (26 chars Crockford Base32)"
        );
    }
}
