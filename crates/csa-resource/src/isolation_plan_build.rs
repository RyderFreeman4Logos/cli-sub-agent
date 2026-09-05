//! Final validation and activation of the fully owned isolation plan.

use super::*;

impl IsolationPlanBuilder {
    /// Consume the builder and produce an [`IsolationPlan`].
    ///
    /// # Errors
    ///
    /// Returns an error when filesystem enforcement is `Required` but the
    /// filesystem capability is `None`, or bind pinning/capability validation fails.
    pub fn build(mut self) -> anyhow::Result<IsolationPlan> {
        if let Some(error) = self.preflight_error.take() {
            return Err(error);
        }
        // Filesystem enforcement: use dedicated override if set, otherwise
        // inherit from the resource enforcement mode.
        let fs_mode = self.fs_enforcement_mode.unwrap_or(self.enforcement_mode);

        match fs_mode {
            EnforcementMode::Off => {
                self.filesystem = FilesystemCapability::None;
            }
            EnforcementMode::Required => {
                if self.filesystem == FilesystemCapability::None {
                    anyhow::bail!("filesystem isolation required but no capability detected");
                }
            }
            EnforcementMode::BestEffort => {
                if self.filesystem == FilesystemCapability::None {
                    self.degraded_reasons
                        .push("no filesystem isolation available; proceeding without".into());
                }
            }
        }

        // Resource enforcement: handled separately.
        match self.enforcement_mode {
            EnforcementMode::BestEffort => {
                if self.resource == ResourceCapability::None {
                    self.degraded_reasons
                        .push("no resource isolation available; proceeding without".into());
                }
            }
            EnforcementMode::Off | EnforcementMode::Required => {
                // Required for resources is checked upstream in pipeline_sandbox.
                // Off is a no-op for the resource axis (capabilities are kept as-is
                // because cgroup limits don't need explicit disabling here).
            }
        }

        if self.filesystem != FilesystemCapability::None
            && !self.readonly_project_root
            && let Some(project_root) = self.project_root.as_deref()
            && let Some([git_dir, common_dir]) =
                runtime_path::linked_worktree_git_admin_dirs(project_root)?
        {
            self.writable_paths.extend([common_dir, git_dir]);
        }

        readable::push_runtime_daemon_socket_readable_paths(
            self.filesystem,
            self.user_daemon_ipc,
            &self.writable_paths,
            &mut self.readable_paths,
        );

        if self.filesystem == FilesystemCapability::Bwrap {
            let readonly_project = self
                .project_root
                .as_deref()
                .filter(|_| self.readonly_project_root);
            self.readable_paths
                .extend(crate::bwrap::plan_extra_readonly_paths(
                    &self.writable_paths,
                    readonly_project,
                    std::env::var_os("HOME").as_deref().map(Path::new),
                )?);
        }

        let bind_fd_count = if self.filesystem == FilesystemCapability::Bwrap {
            crate::bwrap::readable_bind_fd_count(
                &self.readable_paths,
                &self.writable_paths,
                self.project_root
                    .as_deref()
                    .filter(|_| self.readonly_project_root),
            )
        } else {
            0
        };
        readable::downgrade_incompatible_cgroup_filesystem(
            &mut self.resource,
            self.filesystem,
            bind_fd_count,
            &mut self.degraded_reasons,
        );

        codex_paths::validate_required_writable_dirs(
            self.filesystem,
            &self.required_writable_dirs,
            &self.writable_paths,
        )?;
        hermes_paths::finish_plan(
            self.filesystem,
            bind_fd_count,
            self.pending_hermes_runtime.take(),
        )?;

        Ok(IsolationPlan {
            resource: self.resource,
            filesystem: self.filesystem,
            writable_paths: self.writable_paths,
            readable_paths: self.readable_paths,
            env_overrides: self.env_overrides,
            degraded_reasons: self.degraded_reasons,
            memory_max_mb: self.memory_max_mb,
            memory_swap_max_mb: self.memory_swap_max_mb,
            pids_max: self.pids_max,
            readonly_project_root: self.readonly_project_root,
            project_root: self.project_root,
            soft_limit_percent: self.soft_limit_percent,
            memory_monitor_interval_seconds: self.memory_monitor_interval_seconds,
            user_daemon_ipc: self.user_daemon_ipc,
        })
    }
}
