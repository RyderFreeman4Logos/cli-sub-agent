//! CLI subcommands for the `csa todo` command group.

use clap::{Subcommand, ValueEnum};

#[derive(Subcommand)]
pub enum TodoCommands {
    /// Create a new TODO plan
    Create {
        /// Plan title
        title: String,

        /// Associate with a git branch (default: current branch; use --no-branch for none)
        #[arg(short, long, conflicts_with = "no_branch")]
        branch: Option<String>,

        /// Do not associate with any git branch (overrides auto-detection)
        #[arg(long)]
        no_branch: bool,

        /// Language for TODO content (e.g., "Chinese (Simplified)", "English")
        #[arg(short, long)]
        language: Option<String>,

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

        /// Show the plan's spec.toml criteria instead of TODO.md
        #[arg(long, conflicts_with_all = ["path", "version"])]
        spec: bool,

        /// Append a reference file listing after TODO.md content
        #[arg(long, conflicts_with_all = ["path", "spec"])]
        refs: bool,

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

    /// Manage reference files attached to TODO plans
    Ref {
        #[command(subcommand)]
        cmd: TodoRefCommands,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum TodoDagFormat {
    Mermaid,
    Terminal,
    Dot,
}

#[derive(Subcommand)]
pub enum TodoRefCommands {
    /// List reference files for a plan
    List {
        /// Timestamp of the TODO plan (default: latest)
        #[arg(short, long)]
        timestamp: Option<String>,

        /// Include token estimates for each reference
        #[arg(long)]
        tokens: bool,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// Show a reference file's content
    Show {
        /// Timestamp of the TODO plan
        #[arg(short, long)]
        timestamp: Option<String>,

        /// Reference filename (e.g., recon-summary.md)
        name: String,

        /// Maximum token budget (error if reference exceeds this)
        #[arg(long, default_value = "8000")]
        max_tokens: usize,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// Add a reference file to a plan
    Add {
        /// Timestamp of the TODO plan
        #[arg(short, long)]
        timestamp: Option<String>,

        /// Reference filename (must end with .md)
        name: String,

        /// Content as inline text
        #[arg(long, conflicts_with = "file")]
        content: Option<String>,

        /// Read content from a file path
        #[arg(long, conflicts_with = "content")]
        file: Option<String>,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// Import a conversation transcript as a reference file
    ImportTranscript {
        /// Timestamp of the TODO plan
        #[arg(short, long)]
        timestamp: Option<String>,

        /// Tool/provider name (claude, codex, gemini, opencode)
        #[arg(long)]
        tool: String,

        /// Session ID to import
        #[arg(long)]
        session: String,

        /// Override reference filename (default: transcript-{tool}-{session_prefix}.md)
        #[arg(long)]
        name: Option<String>,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },
}
