use anyhow::Result;
use csa_config::{GlobalConfig, ProjectConfig};
use csa_process::StreamMode;
use std::path::{Path, PathBuf};

use super::RunLoopRequest;

/// Resolved writer admission inputs retained independently of the run request,
/// whose owned routing fields are moved into the attempt loop.
pub(super) struct RunMemoryAdmission<'a> {
    project_root: &'a Path,
    project_config: Option<&'a ProjectConfig>,
    global_config: &'a GlobalConfig,
    resource_overrides: crate::run_resource_overrides::RunResourceOverrides,
    stream_mode: StreamMode,
    idle_timeout_seconds: u64,
    build_jobs: Option<u32>,
    no_fs_sandbox: bool,
    extra_writable: Vec<PathBuf>,
    extra_readable: Vec<PathBuf>,
}

impl<'a> RunMemoryAdmission<'a> {
    pub(super) fn from_request(request: &RunLoopRequest<'a>) -> Self {
        Self {
            project_root: request.project_root,
            project_config: request.config,
            global_config: request.global_config,
            resource_overrides: request.resource_overrides,
            stream_mode: request.stream_mode,
            idle_timeout_seconds: crate::pipeline::resolve_effective_idle_timeout_seconds(
                request.config,
                request.cli_idle_timeout,
                request.run_timeout_seconds,
            ),
            build_jobs: request.build_jobs,
            no_fs_sandbox: request.no_fs_sandbox,
            extra_writable: request.extra_writable.clone(),
            extra_readable: request.extra_readable.clone(),
        }
    }

    /// Applies the writer role's resolved soft-memory floor before any fresh
    /// run attempt can allocate a session, including native fork children.
    pub(super) fn validate(
        &self,
        tool_name: &str,
        initial_response_timeout_seconds: Option<u64>,
    ) -> Result<csa_resource::ResourceCapability> {
        crate::run_cmd_preflight::validate_run_memory_soft_limit_before_session(
            crate::run_cmd_preflight::RunMemorySoftLimitPreflight {
                project_root: self.project_root,
                project_config: self.project_config,
                global_config: self.global_config,
                tool_name,
                resource_overrides: self.resource_overrides,
                stream_mode: self.stream_mode,
                idle_timeout_seconds: self.idle_timeout_seconds,
                initial_response_timeout_seconds,
                build_jobs: self.build_jobs,
                no_fs_sandbox: self.no_fs_sandbox,
                allow_user_daemon_ipc: false,
                extra_writable: &self.extra_writable,
                extra_readable: &self.extra_readable,
            },
        )
    }

    pub(super) fn validate_host_memory_after_slot_acquisition(
        &self,
        tool_name: &str,
        resource_capability: csa_resource::ResourceCapability,
    ) -> Result<()> {
        crate::run_cmd_preflight::validate_run_host_memory_after_slot_acquisition(
            self.project_root,
            self.project_config,
            tool_name,
            self.resource_overrides,
            resource_capability,
        )
    }
}
