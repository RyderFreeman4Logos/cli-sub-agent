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

        /// Show only page N (positive: from start, negative: from end; -1 = last/current page).
        /// Bypasses the OUTPUT_TOO_LARGE guard.
        #[arg(long, allow_negative_numbers = true)]
        page: Option<i32>,
    },

    /// Search the most recent recorded session for a literal query
    Search {
        /// Literal query string to search for
        query: String,
    },

    /// List compact-event page boundaries in a recorded session
    Pages {
        /// Session ID, `latest`, or a 1-based history index
        session: String,
    },
}
