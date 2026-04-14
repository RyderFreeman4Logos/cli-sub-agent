use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;
use csa_session::state::{MetaSessionState, ToolState};

use super::{TransportMode, TransportOptions, TransportResult};

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

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any;
}
