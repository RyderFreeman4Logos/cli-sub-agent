//! CLI subcommands for the `csa todo` command group.

use clap::{Subcommand, ValueEnum};

#[derive(Subcommand)]
pub enum TodoCommands {
    /// Create a new TODO plan
    Create {
        /// Plan title
        title: String,

        /// Associate with a git branch
        #[arg(short, long)]
        branch: Option<String>,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// Save (git commit) current TODO plan changes
    Save {
        /// Timestamp of the TODO plan (default: latest)
        #[arg(short, long)]
        timestamp: Option<String>,

        /// Commit message
        message: Option<String>,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// Show diff of TODO plan changes
    Diff {
        /// Timestamp of the TODO plan (default: latest)
        #[arg(short, long)]
        timestamp: Option<String>,

        /// Git revision to diff against (default: file's last commit)
        #[arg(long, conflicts_with_all = ["from", "to"])]
        revision: Option<String>,

        /// Diff from this version number (1 = last committed, 2 = before that)
        #[arg(long)]
        from: Option<usize>,

        /// Diff to this version number (1 = last committed, 2 = before that)
        #[arg(long)]
        to: Option<usize>,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// Show git history of a TODO plan
    History {
        /// Timestamp of the TODO plan (default: latest)
        #[arg(short, long)]
        timestamp: Option<String>,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// List all TODO plans for this project
    List {
        /// Filter by status (draft, debating, approved, implementing, done)
        #[arg(long)]
        status: Option<String>,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// Find TODO plans by criteria
    Find {
        /// Filter by branch name
        #[arg(long)]
        branch: Option<String>,

        /// Filter by status
        #[arg(long)]
        status: Option<String>,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// Show a TODO plan's content
    Show {
        /// Timestamp of the TODO plan (default: latest)
        #[arg(short, long)]
        timestamp: Option<String>,

        /// Show a historical version (1 = last committed, 2 = before that)
        #[arg(short, long)]
        version: Option<usize>,

        /// Print only the file path (for scripting)
        #[arg(long, conflicts_with = "version")]
        path: bool,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// Update a TODO plan's status
    Status {
        /// Timestamp of the TODO plan
        timestamp: String,

        /// New status (draft, debating, approved, implementing, done)
        status: String,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// Visualize TODO task dependency DAG
    Dag {
        /// Timestamp of the TODO plan (default: latest)
        #[arg(short, long)]
        timestamp: Option<String>,

        /// DAG output format
        #[arg(long, default_value = "mermaid")]
        format: TodoDagFormat,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum TodoDagFormat {
    Mermaid,
    Terminal,
    Dot,
}
