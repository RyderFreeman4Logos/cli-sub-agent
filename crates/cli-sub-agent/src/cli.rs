use clap::{Parser, Subcommand};
use csa_core::types::{OutputFormat, ToolName};

#[derive(Parser)]
#[command(name = "csa", version)]
#[command(about = "CLI Sub-Agent: Recursive Agent Container")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Output format (text or json)
    #[arg(long, default_value = "text")]
    pub format: OutputFormat,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Execute a task using a specific AI tool
    Run {
        /// Tool to use (gemini-cli, opencode, codex, claude-code)
        #[arg(long, value_enum)]
        tool: ToolName,

        /// Task prompt; reads from stdin if omitted
        prompt: Option<String>,

        /// Resume existing session (ULID or prefix match)
        #[arg(short, long)]
        session: Option<String>,

        /// Human-readable description for a new session
        #[arg(short, long)]
        description: Option<String>,

        /// Parent session ULID (defaults to CSA_SESSION_ID env var)
        #[arg(long, hide = true)]
        parent: Option<String>,

        /// Ephemeral session (no project files, no context injection, auto-cleanup)
        #[arg(long)]
        ephemeral: bool,

        /// Working directory (defaults to CWD)
        #[arg(long)]
        cd: Option<String>,

        /// Model spec: tool/provider/model/thinking_budget
        #[arg(long)]
        model_spec: Option<String>,

        /// Override tool default model (opaque string, passed through to tool)
        #[arg(short, long)]
        model: Option<String>,

        /// Thinking budget (low, medium, high, xhigh)
        #[arg(long)]
        thinking: Option<String>,
    },

    /// Manage sessions
    Session {
        #[command(subcommand)]
        cmd: SessionCommands,
    },

    /// Initialize project configuration (.csa/config.toml)
    Init {
        /// Non-interactive mode
        #[arg(long)]
        non_interactive: bool,
    },

    /// Garbage collect expired locks and empty sessions
    Gc,

    /// Show/manage configuration
    Config {
        #[command(subcommand)]
        cmd: ConfigCommands,
    },

    /// Review code changes using an AI tool
    Review(ReviewArgs),
}

#[derive(clap::Args)]
pub struct ReviewArgs {
    /// Tool to use for review (defaults to first enabled tool in config)
    #[arg(long)]
    pub tool: Option<ToolName>,

    /// Resume existing review session
    #[arg(short, long)]
    pub session: Option<String>,

    /// Override model
    #[arg(short, long)]
    pub model: Option<String>,

    /// Review uncommitted changes (git diff)
    #[arg(long)]
    pub diff: bool,

    /// Compare against branch (default: main)
    #[arg(long, default_value = "main")]
    pub branch: String,

    /// Review specific commit
    #[arg(long)]
    pub commit: Option<String>,

    /// Custom review instructions
    pub prompt: Option<String>,

    /// Working directory
    #[arg(long)]
    pub cd: Option<String>,
}

#[derive(Subcommand)]
pub enum SessionCommands {
    /// List available sessions (with tree hierarchy)
    List {
        #[arg(long)]
        cd: Option<String>,

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
}

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Show current configuration
    Show,
    /// Edit configuration with $EDITOR
    Edit,
    /// Validate configuration file
    Validate,
}
