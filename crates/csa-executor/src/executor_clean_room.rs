use super::*;
use crate::command_isolation::{
    CleanRoomCapability, CommandIsolationError, CommandIsolationPolicy,
};

pub(crate) fn validate_clean_room_request(
    extra_env: Option<&HashMap<String, String>>,
    options: &ExecuteOptions,
) -> Result<()> {
    let invalid = if extra_env.is_some() {
        Some("generic extra_env is a second environment authority")
    } else if options.pre_session_hook.is_some() {
        Some("pre-session hooks may mutate exact prompt delivery")
    } else if options.subtree_pin.is_some() {
        Some("subtree pin authority is unavailable in clean-room mode")
    } else if options.allow_git_push {
        Some("git-push authority is unavailable in clean-room mode")
    } else if options.setting_sources.is_some() {
        Some("transport setting sources are unavailable in clean-room mode")
    } else if options
        .sandbox
        .as_ref()
        .is_some_and(|sandbox| sandbox.best_effort)
    {
        Some("best-effort sandbox fallback is unavailable in clean-room mode")
    } else if options
        .sandbox
        .as_ref()
        .is_some_and(|sandbox| !sandbox.isolation_plan.degraded_reasons.is_empty())
    {
        Some("degraded isolation plans are unavailable in clean-room mode")
    } else {
        None
    };
    if let Some(reason) = invalid {
        return Err(CommandIsolationError::InvalidRequest { reason }.into());
    }
    Ok(())
}

impl Executor {
    /// Execute using an explicit legacy or clean-room command policy.
    #[tracing::instrument(skip_all, fields(tool = %self.tool_name()))]
    #[allow(clippy::too_many_arguments)]
    pub async fn execute_with_command_isolation(
        &self,
        prompt: &str,
        tool_state: Option<&ToolState>,
        session: &MetaSessionState,
        extra_env: Option<&HashMap<String, String>>,
        options: ExecuteOptions,
        session_config: Option<SessionConfig>,
        policy: CommandIsolationPolicy,
    ) -> Result<TransportResult> {
        if matches!(policy, CommandIsolationPolicy::Legacy) {
            return self
                .execute_with_transport(
                    prompt,
                    tool_state,
                    session,
                    extra_env,
                    options,
                    session_config,
                )
                .await;
        }

        validate_clean_room_request(extra_env, &options)?;
        if !matches!(self, Self::Codex { .. } | Self::Opencode { .. }) {
            return Err(CommandIsolationError::Unsupported {
                reason: "clean-room execution is supported only for direct Codex and OpenCode routes",
            }
            .into());
        }
        if matches!(self, Self::Codex { .. }) && self.codex_tmux_mode_enabled() {
            return Err(CommandIsolationError::InvalidRequest {
                reason: "Codex tmux wrapping is unavailable in clean-room mode",
            }
            .into());
        }

        let transport = self.transport(session_config)?;
        if let CleanRoomCapability::Unsupported { reason } = transport.clean_room_capability() {
            return Err(CommandIsolationError::Unsupported { reason }.into());
        }

        let sandbox_transport = options
            .sandbox
            .as_ref()
            .map(|context| SandboxTransportConfig {
                isolation_plan: context.isolation_plan.clone(),
                tool_name: context.tool_name.clone(),
                session_id: context.session_id.clone(),
                best_effort: context.best_effort,
            });
        let transport_options = TransportOptions {
            stream_mode: options.stream_mode,
            idle_timeout_seconds: options.idle_timeout_seconds,
            acp_crash_max_attempts: options.acp_crash_max_attempts,
            initial_response_timeout: ResolvedTimeout(options.initial_response_timeout_seconds),
            liveness_dead_seconds: options.liveness_dead_seconds,
            stdin_write_timeout_seconds: options.stdin_write_timeout_seconds,
            acp_init_timeout_seconds: options.acp_init_timeout_seconds,
            termination_grace_period_seconds: options.termination_grace_period_seconds,
            output_spool: options.output_spool.as_deref(),
            output_spool_max_bytes: options.output_spool_max_bytes,
            output_spool_keep_rotated: options.output_spool_keep_rotated,
            error_marker_scan_enabled: options.error_marker_scan_enabled,
            setting_sources: None,
            sandbox: sandbox_transport.as_ref(),
            thinking_budget: self.thinking_budget().cloned(),
            subtree_pin: None,
            allow_git_push: false,
        };
        let mut result = transport
            .execute_with_command_isolation(
                prompt,
                tool_state,
                session,
                None,
                transport_options,
                &policy,
            )
            .await?;
        result.execution.consolidate_stderr_retries();
        Ok(result)
    }
}
