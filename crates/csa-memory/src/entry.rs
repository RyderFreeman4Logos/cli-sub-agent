use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: Ulid,
    pub timestamp: DateTime<Utc>,
    pub project: Option<String>,
    pub tool: Option<String>,
    pub session_id: Option<String>,
    pub tags: Vec<String>,
    pub content: String,
    pub facts: Vec<String>,
    pub source: MemorySource,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MemorySource {
    PostRun,
    Manual,
    Consolidated,
}
