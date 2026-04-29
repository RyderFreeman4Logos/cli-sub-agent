pub mod consolidation;
mod entry;
mod ephemeral_client;
mod index;
mod llm_client;
mod mempal_detect;
mod noop_client;
mod resolve_backend;
mod store;

pub use consolidation::{ConsolidationPlan, MergeGroup, execute_consolidation, plan_consolidation};
pub use entry::{MemoryEntry, MemorySource};
pub use ephemeral_client::EphemeralClient;
pub use index::{MemoryIndex, SearchResult};
pub use llm_client::{ApiClient, Fact, MemoryLlmClient, ModelRotator};
pub use mempal_detect::{MempalInfo, detect_mempal};
pub use noop_client::NoopClient;
pub use resolve_backend::resolve_backend;
pub use store::{MemoryFilter, MemoryStore, append_entry, list_entries, quick_search};
