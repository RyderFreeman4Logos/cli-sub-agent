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
        #[arg(default_value = "latest")]
        session: String,

        /// Page number (newest-first): 0 = current page (after last compact),
        /// 1 = previous page, 2 = before that, and so on.
        /// Bypasses the OUTPUT_TOO_LARGE guard.
        #[arg(long)]
        page: Option<u32>,
    },

    /// Search the most recent recorded session for a literal query
    Search {
        /// Literal query string to search for
        query: String,
    },

    /// List compact-event page boundaries (newest-first).
    ///
    /// Page 0 is the content after the most recent compact event (the
    /// "current" page).  Page 1 is between the second-to-last and last
    /// compact, and so on.
    Pages {
        /// Session ID, `latest`, or a 1-based history index
        #[arg(default_value = "latest")]
        session: String,
    },
}
