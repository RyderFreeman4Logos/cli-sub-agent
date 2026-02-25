use anyhow::{Result, bail};
use async_trait::async_trait;

use crate::{Fact, MemoryEntry, MemoryLlmClient};

#[derive(Debug, Default, Clone, Copy)]
pub struct EphemeralClient;

#[async_trait]
impl MemoryLlmClient for EphemeralClient {
    async fn extract_facts(&self, _text: &str) -> Result<Vec<Fact>> {
        bail!("ephemeral client not yet wired")
    }

    async fn summarize(&self, _entries: &[MemoryEntry]) -> Result<String> {
        bail!("ephemeral client not yet wired")
    }
}
