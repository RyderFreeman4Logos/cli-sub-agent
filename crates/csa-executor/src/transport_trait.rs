use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use async_trait::async_trait;
use csa_session::state::{MetaSessionState, ToolState};

use super::{ResolvedTimeout, TransportMode, TransportOptions, TransportResult};

#[async_trait]
pub trait Transport: Send + Sync {
    fn mode(&self) -> TransportMode;

    async fn execute(
        &self,
        prompt: &str,
        tool_state: Option<&ToolState>,
        session: &MetaSessionState,
        extra_env: Option<&HashMap<String, String>>,
        options: TransportOptions<'_>,
    ) -> Result<TransportResult>;

    async fn execute_in(
        &self,
        prompt: &str,
        work_dir: &Path,
        extra_env: Option<&HashMap<String, String>>,
        stream_mode: csa_process::StreamMode,
        idle_timeout_seconds: u64,
        initial_response_timeout: ResolvedTimeout,
    ) -> Result<TransportResult>;

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any;
}
