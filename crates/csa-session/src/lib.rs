//! Session management with ULID-based genealogy tracking.

pub mod checkpoint;
pub mod event_writer;
pub mod finding_id;
pub mod genealogy;
pub mod git;
pub mod manager;
pub mod metadata;
pub mod output_parser;
pub mod output_section;
pub mod redact;
pub mod review_artifact;
pub mod result;
pub mod soft_fork;
pub mod state;
pub mod validate;

// Re-export key types
pub use state::{
    ContextStatus, Genealogy, MetaSessionState, PhaseEvent, SandboxInfo, SessionPhase, TaskContext,
    TokenUsage, ToolState,
};

pub use metadata::SessionMetadata;

pub use event_writer::{EventWriteStats, EventWriter};
pub use finding_id::{FindingId, anchor_hash, normalize_path};
pub use output_parser::{
    estimate_tokens, load_output_index, persist_structured_output, read_all_sections, read_section,
};
pub use output_section::{OutputIndex, OutputSection};
pub use redact::{redact_event, redact_text_content};
pub use review_artifact::{Finding, ReviewArtifact, Severity, SeveritySummary};
pub use result::{SessionArtifact, SessionResult};
pub use soft_fork::{SoftForkContext, soft_fork_session};

// Re-export manager functions
pub use manager::{
    complete_session, create_session, delete_session, delete_session_from_root, detect_git_head,
    find_sessions, get_session_dir, get_session_root, list_all_sessions, list_artifacts,
    list_sessions, list_sessions_from_root, list_sessions_from_root_readonly, load_metadata,
    load_result, load_session, resolve_fork_source, resolve_resume_session, save_result,
    save_session, save_session_in, update_last_accessed, validate_tool_access,
};

pub use manager::ResumeSessionResolution;

// Re-export genealogy functions
pub use genealogy::{find_children, list_sessions_tree, list_sessions_tree_filtered};

// Re-export validation functions
pub use validate::{new_session_id, resolve_session_prefix, validate_session_id};
