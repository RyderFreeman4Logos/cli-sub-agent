//! Session management with ULID-based genealogy tracking.

pub mod adjudication;
pub mod checkpoint;
pub mod cooldown;
pub mod event_writer;
pub mod finding_id;
pub mod genealogy;
pub mod git;
pub mod manager;
pub mod metadata;
pub mod output_parser;
pub mod output_section;
pub mod redact;
pub mod result;
pub mod review_artifact;
pub mod soft_fork;
pub mod state;
pub mod tool_output_store;
pub mod validate;
pub mod vcs_backends;

#[cfg(test)]
#[path = "vcs_identity_tests.rs"]
mod vcs_identity_tests;

/// Shared test-only environment lock.
///
/// All tests that mutate process-wide environment variables (e.g.
/// `XDG_STATE_HOME`, `CSA_DAEMON_*`) **must** acquire this lock to prevent
/// data races between test threads.  Previously `manager_tests` and
/// `genealogy::tests` each had their own static lock, which did not
/// protect against cross-module races.
#[cfg(test)]
pub(crate) mod test_env {
    use std::sync::{LazyLock, Mutex};

    pub static TEST_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
}

// Re-export cooldown types
pub use cooldown::{
    CooldownAction, CooldownMarker, compute_cooldown_wait, evaluate_cooldown, read_cooldown_marker,
    write_cooldown_marker, write_cooldown_marker_from_session_dir,
};

// Re-export key types
pub use adjudication::{AdjudicationRecord, AdjudicationSet, Verdict};
pub use state::{
    ContextStatus, Genealogy, MetaSessionState, PhaseEvent, ReviewSessionMeta, SandboxInfo,
    SessionPhase, TaskContext, TokenUsage, ToolState, write_review_meta,
};

pub use metadata::SessionMetadata;

pub use event_writer::{EventWriteStats, EventWriter};
pub use finding_id::{FindingId, anchor_hash, normalize_path};
pub use output_parser::{
    estimate_tokens, load_output_index, parse_return_packet, persist_structured_output,
    persist_structured_output_from_file, read_all_sections, read_section,
    validate_return_packet_path,
};
pub use output_section::{
    ChangedFile, FileAction, OutputIndex, OutputSection, RETURN_PACKET_MAX_SUMMARY_CHARS,
    RETURN_PACKET_SECTION_ID, ReturnPacket, ReturnPacketRef, ReturnStatus,
};
pub use redact::{redact_event, redact_text_content};
pub use result::{SessionArtifact, SessionResult};
pub use review_artifact::{Finding, ReviewArtifact, Severity, SeveritySummary};
pub use soft_fork::{SoftForkContext, soft_fork_session};
pub use vcs_backends::{GitBackend, JjBackend, create_vcs_backend};

// Re-export manager functions
pub use manager::{
    complete_session, create_session, delete_session, delete_session_from_root, detect_git_head,
    find_sessions, get_session_dir, get_session_dir_global, get_session_root,
    list_all_project_session_roots, list_all_sessions, list_all_sessions_all_projects,
    list_artifacts, list_sessions, list_sessions_from_root, list_sessions_from_root_readonly,
    load_metadata, load_result, load_session, load_session_global_exact, resolve_fork_source,
    resolve_resume_session, save_result, save_session, save_session_in, update_last_accessed,
    validate_tool_access,
};

pub use manager::ResumeSessionResolution;

// Re-export genealogy functions
pub use genealogy::{find_children, list_sessions_tree, list_sessions_tree_filtered};

// Re-export validation functions
pub use validate::{new_session_id, resolve_session_prefix, validate_session_id};

/// Controls how broadly session lookup scans across project boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionLookupScope {
    /// Default: prefix match allowed, mutations allowed, project-scoped.
    CurrentProject,
    /// Exact 26-char ULID only, read-only operations.
    /// When not found in current project, scans ALL project dirs
    /// in the state directory. Bypasses project path validation.
    GlobalExact,
    /// For `session list --all-projects`: enumerate sessions from all project directories.
    AllProjects,
}
