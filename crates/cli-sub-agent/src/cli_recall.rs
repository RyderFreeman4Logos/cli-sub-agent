//! CLI subcommand for recall-based main-agent context recovery.

use clap::{Args, Subcommand};

#[derive(Args)]
pub struct RecallArgs {
    #[command(subcommand)]
    pub cmd: RecallCommands,
}

#[derive(Subcommand)]
pub enum RecallCommands {
    /// List recently recorded main-agent sessions
    List {
        /// Show only the N most recent sessions
        #[arg(long, default_value = "10")]
        limit: usize,
    },

    /// Render a recorded session as markdown
    Read {
        /// Session ID, `latest`, or a 1-based history index
        session: String,
    },

    /// Search the most recent recorded session for a literal query
    Search {
        /// Literal query string to search for
        query: String,
    },
}
