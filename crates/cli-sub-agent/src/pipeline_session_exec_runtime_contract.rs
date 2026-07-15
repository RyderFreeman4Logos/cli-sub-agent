use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;
use csa_executor::{ExecuteOptions, Executor, SessionConfig};
use csa_hooks::HooksConfig;
use csa_session::ToolState;

use crate::edit_restriction_guard::{NewFileGuard, TrackedFileEditGuard};
use crate::pipeline::MemoryInjectionOptions;
use crate::startup_env::StartupSubtreeEnv;

pub(super) struct SessionRuntimeInput<'a> {
    pub(super) executor: &'a Executor,
    pub(super) tool: &'a ToolName,
    pub(super) prompt: &'a str,
    pub(super) session_arg: Option<&'a str>,
    pub(super) fresh_spawn_preflight_override: bool,
    pub(super) project_root: &'a Path,
    pub(super) session_dir: &'a Path,
    pub(super) config: Option<&'a ProjectConfig>,
    pub(super) extra_env: Option<&'a HashMap<String, String>>,
    pub(super) subtree_pin: Option<&'a csa_core::env::SubtreeModelPin>,
    pub(super) allow_git_push: bool,
    pub(super) task_type: Option<&'a str>,
    pub(super) context_load_options: Option<&'a csa_executor::ContextLoadOptions>,
    pub(super) stream_mode: csa_process::StreamMode,
    pub(super) idle_timeout_seconds: u64,
    pub(super) initial_response_timeout_seconds: Option<u64>,
    pub(super) wall_timeout: Option<std::time::Duration>,
    pub(super) memory_injection: Option<&'a MemoryInjectionOptions>,
    pub(super) global_config: Option<&'a GlobalConfig>,
    pub(super) pre_session_hook: Option<csa_hooks::PreSessionHookInvocation>,
    pub(super) resource_overrides: crate::run_resource_overrides::RunResourceOverrides,
    pub(super) no_fs_sandbox: bool,
    pub(super) allow_user_daemon_ipc: bool,
    pub(super) readonly_project_root: bool,
    pub(super) extra_writable: &'a [PathBuf],
    pub(super) extra_readable: &'a [PathBuf],
    pub(super) error_marker_scan_override: Option<bool>,
    pub(super) cli_no_hook_bypass_scan: bool,
    pub(super) startup_env: &'a StartupSubtreeEnv,
    pub(super) resolved_provider_session_id: &'a Option<String>,
    pub(super) memory_project_key: Option<&'a str>,
}

pub(super) struct SessionRuntimePlan {
    pub(super) effective_prompt: String,
    pub(super) tool_state: Option<ToolState>,
    pub(super) execute_options: ExecuteOptions,
    pub(super) session_config: Option<SessionConfig>,
    pub(super) completion: SessionCompletionPlan,
}

pub(super) struct SessionCompletionPlan {
    pub(super) merged_env: HashMap<String, String>,
    pub(super) hooks_config: HooksConfig,
    pub(super) sessions_root: String,
    pub(super) edit_guard: Option<TrackedFileEditGuard>,
    pub(super) new_file_guard: Option<NewFileGuard>,
    pub(super) result_file_cleared: bool,
    pub(super) execution_start_time: chrono::DateTime<chrono::Utc>,
    pub(super) commit_guard_enabled: bool,
    pub(super) require_commit_on_mutation: bool,
    pub(super) hook_bypass_scan_enabled: bool,
    pub(super) is_git: bool,
    pub(super) inside_git_worktree: bool,
    pub(super) pre_run_workspace: Option<crate::run_cmd::GitWorkspaceSnapshot>,
    pub(super) pre_exec_snapshot: Option<crate::pipeline_post_exec::PreExecutionSnapshot>,
    pub(super) timeout_diagnostics: Option<crate::session_kill_diagnostics::TimeoutDiagnostics>,
    pub(super) sa_mode: bool,
}

impl SessionCompletionPlan {
    pub(super) fn merged_env_ref(&self) -> Option<&HashMap<String, String>> {
        (!self.merged_env.is_empty()).then_some(&self.merged_env)
    }
}
