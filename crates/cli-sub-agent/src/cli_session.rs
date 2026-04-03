//! CLI subcommands for the `csa session` command group.

use clap::Subcommand;

#[derive(Subcommand)]
pub enum SessionCommands {
    /// List available sessions (with tree hierarchy)
    List {
        #[arg(long)]
        cd: Option<String>,

        /// Filter by git branch
        #[arg(long)]
        branch: Option<String>,

        /// Filter by tool (comma-separated)
        #[arg(long)]
        tool: Option<String>,

        /// Show tree structure
        #[arg(long)]
        tree: bool,
    },

    /// Compress session context (gemini-cli: /compress, others: /compact)
    Compress {
        #[arg(short, long)]
        session: String,

        /// Working directory (defaults to CWD)
        #[arg(long)]
        cd: Option<String>,
    },

    /// Delete a session
    Delete {
        #[arg(short, long)]
        session: String,

        /// Working directory (defaults to CWD)
        #[arg(long)]
        cd: Option<String>,
    },

    /// Remove sessions older than N days
    Clean {
        /// Remove sessions not accessed within N days
        #[arg(long)]
        days: u64,

        /// Show what would be removed without actually removing
        #[arg(long)]
        dry_run: bool,

        /// Filter by tool (comma-separated)
        #[arg(long)]
        tool: Option<String>,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// View session logs
    Logs {
        /// Session ULID or prefix
        #[arg(short, long)]
        session: String,

        /// Show only last N lines
        #[arg(long)]
        tail: Option<usize>,

        /// Show ACP JSONL events from output/acp-events.jsonl
        #[arg(long)]
        events: bool,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// Check whether a session is still alive using filesystem liveness signals
    IsAlive {
        /// Session ID or prefix
        #[arg(short, long)]
        session: String,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// Show the last execution result for a session
    Result {
        /// Session ID or prefix
        #[arg(short, long)]
        session: String,

        /// Output as JSON instead of human-readable
        #[arg(long)]
        json: bool,

        /// Show only the summary section of structured output
        #[arg(long, conflicts_with_all = ["section", "full"])]
        summary: bool,

        /// Show a specific section by ID (e.g., "details", "implementation")
        #[arg(long, conflicts_with_all = ["summary", "full"])]
        section: Option<String>,

        /// Show all structured output sections in order
        #[arg(long, conflicts_with_all = ["summary", "section"])]
        full: bool,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// List artifacts in a session's output directory
    Artifacts {
        /// Session ID or prefix
        #[arg(short, long)]
        session: String,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// Show git history for a session
    Log {
        /// Session ID or prefix
        #[arg(short, long)]
        session: String,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// Write a checkpoint note (git notes) for audit trail
    Checkpoint {
        /// Session ID or prefix
        #[arg(short, long)]
        session: String,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// List all checkpoint notes
    Checkpoints {
        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// Measure token savings from structured output
    Measure {
        /// Session ID or prefix
        #[arg(short, long)]
        session: String,

        /// Output as JSON instead of human-readable
        #[arg(long)]
        json: bool,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// Retrieve a compressed tool output from a session
    ToolOutput {
        /// Session ID or prefix
        session: String,

        /// Tool output index to retrieve (omit with --list to show manifest)
        #[arg(conflicts_with = "list")]
        index: Option<u32>,

        /// List all compressed tool outputs (show manifest)
        #[arg(long)]
        list: bool,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// Wait for a daemon session to complete (poll until result exists and the daemon exits).
    /// Hardcoded timeout: 250 seconds; prints stdout.log plus a completion marker.
    Wait {
        /// Session ID to wait for
        #[arg(long)]
        session: String,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// Kill a running daemon session (SIGTERM, then SIGKILL after grace period)
    Kill {
        /// Session ID to kill
        #[arg(long)]
        session: String,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// Attach to a running daemon session (tail stdout/stderr until the daemon exits)
    Attach {
        /// Session ID to attach to
        #[arg(long)]
        session: String,

        /// Show stderr alongside stdout
        #[arg(long)]
        stderr: bool,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },
}
