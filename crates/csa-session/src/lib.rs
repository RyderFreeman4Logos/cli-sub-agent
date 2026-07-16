//! Session management with ULID-based genealogy tracking.

mod atomic_state_write;
mod session_output_artifact;

pub mod adjudication;
pub mod caller_detect;
pub mod checklist_store;
pub mod checkpoint;
pub mod convergence;
pub mod cooldown;
pub mod event_writer;
pub mod finding_id;
pub mod genealogy;
pub mod git;
pub mod jj_journal;
pub mod kill_diagnostics;
pub mod large_diff_warning;
pub mod manager;
pub mod metadata;
pub mod output_parser;
pub mod output_section;
pub mod post_exec_gate_report;
mod process_tree_memory;
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

#[cfg(test)]
#[path = "convergence_tests.rs"]
mod convergence_tests;

#[cfg(test)]
#[path = "convergence_authority_tests.rs"]
mod convergence_authority_tests;

#[cfg(test)]
#[path = "convergence_ledger_tests.rs"]
mod convergence_ledger_tests;

#[cfg(test)]
#[path = "convergence_evidence_tests.rs"]
mod convergence_evidence_tests;

#[cfg(test)]
#[path = "convergence_model_evidence_tests.rs"]
mod convergence_model_evidence_tests;

#[cfg(test)]
#[path = "convergence_protocol_tests.rs"]
mod convergence_protocol_tests;

#[cfg(test)]
#[path = "convergence_repair_tests.rs"]
mod convergence_repair_tests;

#[cfg(test)]
#[path = "convergence_discovery_tests.rs"]
mod convergence_discovery_tests;

#[cfg(test)]
#[path = "convergence_store_tests.rs"]
mod convergence_store_tests;

#[cfg(test)]
#[path = "convergence_store_review_tests.rs"]
mod convergence_store_review_tests;

#[cfg(test)]
#[path = "convergence_attestation_tests.rs"]
mod convergence_attestation_tests;

#[cfg(test)]
#[path = "convergence_action_journal_tests.rs"]
mod convergence_action_journal_tests;

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
    sessions_dir_for_project, write_cooldown_marker, write_cooldown_marker_for_project,
    write_cooldown_marker_from_session_dir,
};

// Re-export key types
pub use adjudication::{AdjudicationRecord, AdjudicationSet, Verdict};
pub use caller_detect::{CallerSessionInfo, detect_caller_session};
pub use checklist_store::ChecklistStore;
pub use convergence::{
    AdmittedModelIdentity, ArtifactEvidenceRef, AttestationArtifactReader,
    AttestationBindingDigests, CLEAN_ROOM_REVIEW_SCHEMA_ID,
    COMPLETION_ACTION_JOURNAL_SCHEMA_VERSION, CONVERGENCE_LEDGER_SCHEMA_VERSION, CampaignId,
    CampaignRecord, CandidateDisposition, CandidateDispositionRecord, CandidateId, CandidateRecord,
    CleanRoomReviewArtifactBindings, CleanRoomReviewRecord, CommandAuthorityCatalogIdentity,
    CommandAuthorityPolicy, CommandAuthoritySnapshot, CommandAuthoritySource,
    CompletionActionClaim, CompletionActionId, CompletionActionJournal,
    CompletionActionJournalError, CompletionActionJournalRead, CompletionActionJournalStoreError,
    CompletionActionRecord, CompletionActionState, ConsolidatedRepairAuthorization,
    ConvergenceAppendError, ConvergenceEvent, ConvergenceLedger, ConvergenceLedgerEntry,
    ConvergenceLedgerStore, CoverageCellId, CoverageCellRecord, CoverageDispositionRecord,
    CoveragePlanFinalizationRecord, CoverageRequirement, CoverageScope, CsaSessionId,
    DiscoveryAttemptFinalizationRecord, DiscoveryAttemptId, DiscoveryAttemptRecord,
    DiscoveryDirective, DiscoveryRunIntent, EpochId, EpochRecord, GATE_EVIDENCE_SCHEMA_ID,
    GateCommandResult, GateEvidenceRecord, GitObjectId, IndependentlyVerifiedModel,
    LEGACY_CLEAN_ROOM_REVIEW_SCHEMA_ID, LEGACY_COMPLETION_ACTION_JOURNAL_SCHEMA_VERSION,
    LedgerEventId, MAX_REPAIR_INTENT_BATCHES, MERGE_ATTESTATION_SCHEMA_ID, MergeAttestationRecord,
    ModelEvidence, ModelEvidenceConfidence, ModelEvidenceProvenance, ObservedToolEvidence,
    ProviderTurnExecutionId, ProviderTurnExecutionRecord, ProviderTurnExecutionState,
    ProviderTurnReservation, REPAIR_INTENT_SCHEMA_VERSION, RepairBatchId, RepairBatchRecord,
    RepairHandoffId, RepairHandoffRecord, RepairIntent, RepairIntentState, RootClusterId,
    RootClusterRecord, SemanticFindingIdentity, SemanticLens, SessionRelativeArtifactPath,
    Sha256Digest, StableFindingId, authorize_consolidated_repairs, compute_attestation_bindings,
    next_discovery_directive, parse_legacy_completion_action_journal, verify_merge_attestation,
};
pub use state::{
    ContextStatus, FixConvergenceMeta, Genealogy, MetaSessionState, PhaseEvent, ReviewSessionMeta,
    SandboxInfo, SessionPhase, TaskContext, TokenUsage, ToolState, write_review_meta,
};

pub use metadata::SessionMetadata;

pub use event_writer::{EventWriteStats, EventWriter};
pub use finding_id::{FindingId, anchor_hash, normalize_path};
pub use jj_journal::JjJournal;
pub use kill_diagnostics::KillDiagnosticReport;
pub use large_diff_warning::LargeDiffWarningReport;
pub use output_parser::{
    estimate_tokens, load_output_index, parse_return_packet, persist_structured_output,
    persist_structured_output_from_file, read_all_sections, read_section,
    validate_return_packet_path,
};
pub use output_section::{
    ChangedFile, FileAction, OutputIndex, OutputSection, RETURN_PACKET_MAX_SUMMARY_CHARS,
    RETURN_PACKET_SECTION_ID, ReturnPacket, ReturnPacketRef, ReturnStatus,
};
pub use post_exec_gate_report::{
    GATE_FAILURE_LOG_REL_PATH, GATE_OUTPUT_TAIL_MAX_BYTES, GATE_OUTPUT_TAIL_MAX_LINES,
    GATE_SUMMARY_LEAD, PostExecGateReport, bound_output_tail, parse_failing_step,
    parse_nextest_failing_tests, post_exec_gate_failure_label, post_exec_gate_failure_summary,
};
pub use process_tree_memory::{SessionTreeMemorySampler, session_tree_rss_mb};
pub use redact::{redact_event, redact_text_content};
pub use result::{
    MemorySoftLimitRecoveryDiagnostic, NO_PROVIDER_LAUNCH_ARTIFACT_PATH,
    NO_PROVIDER_LAUNCH_SCHEMA_VERSION, NoProviderLaunchDiagnostic,
    NoProviderLaunchMemoryDiagnostic, RequireCommitRecoveryDiagnostic, SessionArtifact,
    SessionManagerFields, SessionResult, UncommittedChanges, read_no_provider_launch_diagnostic,
    write_no_provider_launch_diagnostic,
};
pub use review_artifact::{
    Finding, FindingsFile, REVIEW_VERDICT_SCHEMA_VERSION, ReviewArtifact, ReviewDiffSize,
    ReviewFinding, ReviewFindingFileRange, ReviewVerdictArtifact, Severity, SeveritySummary,
    write_findings_toml, write_review_verdict,
};
pub use session_output_artifact::{publish_session_output_artifact, read_session_output_artifact};
pub use soft_fork::{SoftForkContext, soft_fork_session};
pub use vcs_backends::{GitBackend, JjBackend, create_vcs_backend};

// Re-export manager functions
pub use manager::{
    CONTRACT_RESULT_ARTIFACT_PATH, LEGACY_USER_RESULT_ARTIFACT_PATH, RESULT_TOML_PATH_CONTRACT_ENV,
    RepoWriteAudit, SaveOptions, SignalResultMetadata, clear_manager_sidecar, complete_session,
    compute_repo_write_audit, contract_result_path, create_session, create_session_fresh,
    create_session_with_daemon_env, decode_session_created_at, delete_session,
    delete_session_from_root, detect_git_head, existing_next_turn_contract_result_artifact_path,
    existing_turn_contract_result_artifact_path, find_sessions, get_session_dir,
    get_session_dir_global, get_session_dir_global_durable, get_session_root,
    is_manager_result_artifact_path, latest_manager_result_artifact_path, legacy_user_result_path,
    list_all_project_session_roots, list_all_sessions, list_all_sessions_all_projects,
    list_artifacts, list_sessions, list_sessions_from_root, list_sessions_from_root_readonly,
    list_sessions_readonly, load_metadata, load_result, load_result_view, load_session,
    load_session_global_exact, next_turn_contract_result_artifact_path,
    next_turn_contract_result_path, observed_session_artifact, redact_result_sidecar_value,
    render_redacted_result_sidecar, resolve_fork_source, resolve_resume_session, save_result,
    save_result_with_options, save_result_with_signal_metadata, save_session, save_session_in,
    turn_contract_result_artifact_path, turn_contract_result_path, update_last_accessed,
    validate_tool_access, write_audit_warning_artifact,
};
pub use manager::{
    RESUME_TARGET_FILE_NAME, ResumeTargetResolution, read_resume_target_from_dir,
    resolve_resume_target_from_dir, write_resume_target,
};

pub use manager::ResumeSessionResolution;
pub use manager::SessionResultView;

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
