//! Session management with ULID-based genealogy tracking.

pub mod genealogy;
pub mod manager;
pub mod state;
pub mod validate;

// Re-export key types
pub use state::{ContextStatus, Genealogy, MetaSessionState, TokenUsage, ToolState};

// Re-export manager functions
pub use manager::{
    create_session, delete_session, get_session_dir, get_session_root, list_all_sessions,
    list_sessions, load_session, save_session, update_last_accessed,
};

// Re-export genealogy functions
pub use genealogy::{find_children, list_sessions_tree};

// Re-export validation functions
pub use validate::{new_session_id, resolve_session_prefix, validate_session_id};
