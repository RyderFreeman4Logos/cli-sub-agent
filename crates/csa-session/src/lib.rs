//! Session management with ULID-based genealogy tracking.

pub mod genealogy;
pub mod git;
pub mod manager;
pub mod metadata;
pub mod result;
pub mod state;
pub mod validate;

// Re-export key types
pub use state::{
    ContextStatus, Genealogy, MetaSessionState, PhaseEvent, SessionPhase, TaskContext, TokenUsage,
    ToolState,
};

pub use metadata::SessionMetadata;

pub use result::SessionResult;

// Re-export manager functions
pub use manager::{
    complete_session, create_session, delete_session, delete_session_from_root, get_session_dir,
    get_session_root, list_all_sessions, list_artifacts, list_sessions, list_sessions_from_root,
    list_sessions_from_root_readonly, load_metadata, load_result, load_session, save_result,
    save_session, save_session_in, update_last_accessed, validate_tool_access,
};

// Re-export genealogy functions
pub use genealogy::{find_children, list_sessions_tree};

// Re-export validation functions
pub use validate::{new_session_id, resolve_session_prefix, validate_session_id};
