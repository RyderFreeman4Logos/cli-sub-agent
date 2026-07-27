use std::path::{Path, PathBuf};

use csa_config::{GlobalConfig, ProjectConfig};
use csa_executor::Executor;
use csa_session::SessionArtifact;

/// All inputs needed for post-execution processing.
pub(crate) struct PostExecContext<'a> {
    pub executor: &'a Executor,
    pub prompt: &'a str,
    pub effective_prompt: &'a str,
    pub task_type: Option<&'a str>,
    pub readonly_project_root: bool,
    pub project_root: &'a Path,
    pub config: Option<&'a ProjectConfig>,
    pub global_config: Option<&'a GlobalConfig>,
    pub session_dir: PathBuf,
    pub sessions_root: String,
    pub execution_start_time: chrono::DateTime<chrono::Utc>,
    pub hooks_config: &'a csa_hooks::HooksConfig,
    pub memory_project_key: Option<String>,
    pub provider_session_id: Option<String>,
    pub events_count: u64,
    pub transcript_artifacts: Vec<SessionArtifact>,
    pub changed_paths: Vec<String>,
    pub pre_exec_snapshot: Option<PreExecutionSnapshot>,
    pub timeout_diagnostics: Option<crate::session_kill_diagnostics::TimeoutDiagnostics>,
    pub has_tool_calls: bool,
    pub turn_count: u32,
    pub output_tokens: Option<u64>,
    pub sa_mode: bool,
    /// The raw exit code as captured BEFORE result-contract enforcement.
    /// Contract validation may call `mark_gate_failure`, coercing `exit_code`
    /// to `1`; the dirty-SA exit-preservation path must see the ORIGINAL
    /// nonzero code (e.g. timeout 124 / signal 137), not the coerced 1
    /// (#2806 R10-F3).
    pub original_exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreExecutionSnapshot {
    pub head: String,
    pub porcelain: Option<String>,
}
