use clap::{Subcommand, ValueEnum};

#[derive(Subcommand, Debug)]
pub enum MemoryCommands {
    /// Search memories using BM25 full-text search
    Search {
        /// Search query
        query: String,
        /// Maximum results
        #[arg(short, long, default_value = "10")]
        limit: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// List memory entries with optional filters
    List {
        /// Filter by project name
        #[arg(long)]
        project: Option<String>,
        /// Filter by tool name
        #[arg(long)]
        tool: Option<String>,
        /// Filter by tag
        #[arg(long)]
        tag: Option<String>,
        /// Only show entries since this date (YYYY-MM-DD)
        #[arg(long)]
        since: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Manually add a memory entry
    Add {
        /// Memory content
        content: String,
        /// Comma-separated tags
        #[arg(long)]
        tags: Option<String>,
    },
    /// Show a specific memory entry by ID
    Show {
        /// Memory entry ULID (prefix match supported)
        id: String,
    },
    /// Clean up old memory entries
    Gc {
        /// Remove entries older than N days
        #[arg(long, default_value = "90")]
        days: u32,
        /// Preview what would be removed
        #[arg(long)]
        dry_run: bool,
    },
    /// Rebuild tantivy search index from JSONL
    Reindex,
    /// Consolidate memory entries via LLM semantic merge
    Consolidate {
        /// Preview consolidation plan without writing changes
        #[arg(long)]
        dry_run: bool,
    },
    /// Migrate legacy memory entries to another backend
    Migrate {
        /// Target memory backend
        #[arg(long, value_enum)]
        to: MemoryMigrationTarget,
        /// Preview migration without invoking the target backend
        #[arg(long)]
        dry_run: bool,
        /// Working directory for target backend ingestion
        #[arg(long)]
        cd: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum MemoryMigrationTarget {
    Mempal,
}
