use anyhow::Result;
use async_trait::async_trait;

use crate::{Fact, MemoryEntry, MemoryLlmClient};

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopClient;

#[async_trait]
impl MemoryLlmClient for NoopClient {
    async fn extract_facts(&self, text: &str) -> Result<Vec<Fact>> {
        Ok(vec![Fact {
            content: text.to_string(),
            tags: Vec::new(),
        }])
    }

    async fn summarize(&self, entries: &[MemoryEntry]) -> Result<String> {
        Ok(entries
            .iter()
            .map(|entry| entry.content.as_str())
            .collect::<Vec<_>>()
            .join("\n"))
    }
}
