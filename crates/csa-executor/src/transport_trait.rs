use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use async_trait::async_trait;
use csa_session::state::{MetaSessionState, ToolState};

use super::{
    ResolvedTimeout, TransportCapabilities, TransportMode, TransportOptions, TransportResult,
};
use crate::command_isolation::{
    CleanRoomCapability, CommandIsolationError, CommandIsolationPolicy,
};

#[async_trait]
pub trait Transport: Send + Sync {
    fn mode(&self) -> TransportMode;

    fn capabilities(&self) -> TransportCapabilities;

    fn clean_room_capability(&self) -> CleanRoomCapability {
        CleanRoomCapability::Unsupported {
            reason: "transport has no native clean-room execution path",
        }
    }

    async fn execute_with_command_isolation(
        &self,
        prompt: &str,
        tool_state: Option<&ToolState>,
        session: &MetaSessionState,
        extra_env: Option<&HashMap<String, String>>,
        options: TransportOptions<'_>,
        policy: &CommandIsolationPolicy,
    ) -> Result<TransportResult> {
        match policy {
            CommandIsolationPolicy::Legacy => {
                self.execute(prompt, tool_state, session, extra_env, options)
                    .await
            }
            CommandIsolationPolicy::CleanRoom(_) => {
                let reason = match self.clean_room_capability() {
                    CleanRoomCapability::Unsupported { reason } => reason,
                    CleanRoomCapability::ExactPromptAndClearedEnvironment => {
                        "transport advertised clean-room support without implementing it"
                    }
                };
                Err(CommandIsolationError::Unsupported { reason }.into())
            }
        }
    }

    async fn execute(
        &self,
        prompt: &str,
        tool_state: Option<&ToolState>,
        session: &MetaSessionState,
        extra_env: Option<&HashMap<String, String>>,
        options: TransportOptions<'_>,
    ) -> Result<TransportResult>;

    #[allow(clippy::too_many_arguments)]
    async fn execute_in(
        &self,
        prompt: &str,
        work_dir: &Path,
        extra_env: Option<&HashMap<String, String>>,
        subtree_pin: Option<&csa_core::env::SubtreeModelPin>,
        allow_git_push: bool,
        stream_mode: csa_process::StreamMode,
        idle_timeout_seconds: u64,
        initial_response_timeout: ResolvedTimeout,
    ) -> Result<TransportResult>;

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any;
}
