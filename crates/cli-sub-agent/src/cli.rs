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
        /// Tool to use (gemini-cli, opencode, codex, claude-code). If omitted, uses tier-based auto-selection.
        #[arg(long, value_enum)]
        tool: Option<ToolName>,

        /// Task prompt; reads from stdin if omitted
        prompt: Option<String>,

        /// Resume existing session (ULID or prefix match)
        #[arg(short, long, conflicts_with = "last")]
        session: Option<String>,

        /// Resume the most recent session for this project
        #[arg(long, conflicts_with_all = ["session", "ephemeral"])]
        last: bool,

        /// Human-readable description for a new session
        #[arg(short, long)]
        description: Option<String>,

        /// Parent session ULID (defaults to CSA_SESSION_ID env var)
        #[arg(long, hide = true)]
        parent: Option<String>,

        /// Ephemeral session (no project files, no context injection, auto-cleanup)
        #[arg(long, conflicts_with = "session")]
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

        /// Generate minimal config (only [project] + detected [tools], no tiers/resources)
        #[arg(long)]
        minimal: bool,
    },

    /// Garbage collect expired locks and empty sessions
    Gc {
        /// Show what would be removed without actually removing
        #[arg(long)]
        dry_run: bool,

        /// Remove sessions not accessed within N days
        #[arg(long)]
        max_age_days: Option<u64>,
    },

    /// Show/manage configuration
    Config {
        #[command(subcommand)]
        cmd: ConfigCommands,
    },

    /// Review code changes using an AI tool
    Review(ReviewArgs),

    /// Check environment and tool availability
    Doctor,

    /// Execute tasks from a batch file
    Batch {
        /// Path to batch TOML file
        file: String,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,

        /// Show what would be executed without running
        #[arg(long)]
        dry_run: bool,
    },

    /// Run as MCP server (JSON-RPC over stdio)
    McpServer,

    /// Manage skills (install, list)
    Skill {
        #[command(subcommand)]
        cmd: SkillCommands,
    },

    /// Setup MCP integration for AI tools
    Setup {
        #[command(subcommand)]
        cmd: SetupCommands,
    },

    /// Update CSA to the latest release
    SelfUpdate {
        /// Check for updates without installing
        #[arg(long)]
        check: bool,
    },
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

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Show current configuration
    Show {
        /// Working directory (defaults to CWD)
        #[arg(long)]
        cd: Option<String>,
    },
    /// Edit configuration with $EDITOR
    Edit {
        /// Working directory (defaults to CWD)
        #[arg(long)]
        cd: Option<String>,
    },
    /// Validate configuration file
    Validate {
        /// Working directory (defaults to CWD)
        #[arg(long)]
        cd: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum SkillCommands {
    /// Install skills from a GitHub repository
    Install {
        /// GitHub repo URL or user/repo format (e.g., "user/repo" or "https://github.com/user/repo")
        source: String,

        /// Target tool to install skills for (claude-code, codex, opencode). Defaults to claude-code.
        #[arg(long)]
        target: Option<String>,
    },

    /// List installed skills
    List,
}

#[derive(Subcommand)]
pub enum SetupCommands {
    /// Setup MCP integration for Claude Code
    ClaudeCode,

    /// Setup MCP integration for OpenAI Codex CLI
    Codex,

    /// Setup MCP integration for OpenCode
    OpenCode,
}
