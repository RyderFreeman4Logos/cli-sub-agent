//! Session management with ULID-based genealogy tracking.

pub mod checkpoint;
pub mod event_writer;
pub mod genealogy;
pub mod git;
pub mod manager;
pub mod metadata;
pub mod redact;
pub mod result;
pub mod state;
pub mod validate;

// Re-export key types
pub use state::{
    ContextStatus, Genealogy, MetaSessionState, PhaseEvent, SandboxInfo, SessionPhase, TaskContext,
    TokenUsage, ToolState,
};

pub use metadata::SessionMetadata;

pub use event_writer::{EventWriteStats, EventWriter};
pub use redact::redact_event;
pub use result::{SessionArtifact, SessionResult};

// Re-export manager functions
pub use manager::{
    complete_session, create_session, delete_session, delete_session_from_root, find_sessions,
    get_session_dir, get_session_root, list_all_sessions, list_artifacts, list_sessions,
    list_sessions_from_root, list_sessions_from_root_readonly, load_metadata, load_result,
    load_session, resolve_resume_session, save_result, save_session, save_session_in,
    update_last_accessed, validate_tool_access,
};

pub use manager::ResumeSessionResolution;

// Re-export genealogy functions
pub use genealogy::{find_children, list_sessions_tree, list_sessions_tree_filtered};

// Re-export validation functions
pub use validate::{new_session_id, resolve_session_prefix, validate_session_id};
