use clap::{Parser, Subcommand, ValueEnum};
use csa_core::types::{OutputFormat, ToolArg, ToolName};

#[derive(Parser)]
#[command(name = "csa", version)]
#[command(about = "CLI Sub-Agent: Recursive Agent Container")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Output format (text or json)
    #[arg(long, global = true, default_value = "text")]
    pub format: OutputFormat,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Execute a task using a specific AI tool
    Run {
        /// Tool selection: auto (default), any-available, or specific tool name
        #[arg(long)]
        tool: Option<ToolArg>,

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

        /// Disable automatic 429 failover to alternative tools
        #[arg(long)]
        no_failover: bool,

        /// Block-wait for a free slot instead of failing when all slots are occupied
        #[arg(long)]
        wait: bool,

        /// Stream child stdout to stderr in real-time (prefix: [stdout])
        #[arg(long)]
        stream_stdout: bool,
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

        /// Scan all projects under ~/.local/state/csa/ (not just current project)
        #[arg(long)]
        global: bool,
    },

    /// Show/manage configuration
    Config {
        #[command(subcommand)]
        cmd: ConfigCommands,
    },

    /// Review code changes using an AI tool
    Review(ReviewArgs),

    /// Run an adversarial debate between heterogeneous AI tools
    Debate(DebateArgs),

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

    /// List and inspect model tiers
    Tiers {
        #[command(subcommand)]
        cmd: TiersCommands,
    },

    /// Setup MCP integration for AI tools
    Setup {
        #[command(subcommand)]
        cmd: SetupCommands,
    },

    /// Manage TODO plans
    Todo {
        #[command(subcommand)]
        cmd: TodoCommands,
    },

    /// Update CSA to the latest release
    SelfUpdate {
        /// Check for updates without installing
        #[arg(long)]
        check: bool,
    },

    /// Route tasks through CSA with Claude model selection and optional skill injection
    #[command(name = "claude-sub-agent")]
    ClaudeSubAgent(ClaudeSubAgentArgs),
}

#[derive(clap::Args)]
pub struct ReviewArgs {
    /// Tool to use for review (defaults to global [review] config or project fallback)
    #[arg(long)]
    pub tool: Option<ToolName>,

    /// Resume existing review session
    #[arg(short, long)]
    pub session: Option<String>,

    /// Override model
    #[arg(short, long)]
    pub model: Option<String>,

    /// Review uncommitted changes (git diff HEAD)
    #[arg(long)]
    pub diff: bool,

    /// Compare against branch (default: main)
    #[arg(long, default_value = "main")]
    pub branch: String,

    /// Review specific commit
    #[arg(long)]
    pub commit: Option<String>,

    /// Review a commit range (e.g., "main...HEAD")
    #[arg(long)]
    pub range: Option<String>,

    /// Review specific files (pathspec)
    #[arg(long)]
    pub files: Option<String>,

    /// Review-and-fix mode (apply fixes directly)
    #[arg(long)]
    pub fix: bool,

    /// Security review mode: auto, on, off
    #[arg(long, default_value = "auto")]
    pub security_mode: String,

    /// Path to context file (e.g., TODO plan)
    #[arg(long)]
    pub context: Option<String>,

    /// Number of reviewers to run in parallel (default: 1)
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
    pub reviewers: u32,

    /// Consensus strategy for multi-reviewer mode
    #[arg(
        long,
        default_value = "majority",
        value_parser = ["majority", "weighted", "unanimous"]
    )]
    pub consensus: String,

    /// Working directory
    #[arg(long)]
    pub cd: Option<String>,
}

#[derive(clap::Args)]
pub struct DebateArgs {
    /// The question or problem to debate; reads from stdin if omitted
    pub question: Option<String>,

    /// Tool to use for debate (overrides auto heterogeneous selection)
    #[arg(long)]
    pub tool: Option<ToolName>,

    /// Resume existing debate session (ULID or prefix match)
    #[arg(short, long)]
    pub session: Option<String>,

    /// Override model
    #[arg(short, long)]
    pub model: Option<String>,

    /// Working directory
    #[arg(long)]
    pub cd: Option<String>,
}

#[derive(clap::Args)]
pub struct ClaudeSubAgentArgs {
    /// Task prompt; reads from stdin if omitted
    pub question: Option<String>,

    /// Tool to use (overrides config-based selection)
    #[arg(long)]
    pub tool: Option<ToolArg>,

    /// Resume existing session
    #[arg(short, long)]
    pub session: Option<String>,

    /// Override model
    #[arg(short, long)]
    pub model: Option<String>,

    /// Model spec override
    #[arg(long)]
    pub model_spec: Option<String>,

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

    /// Show the last execution result for a session
    Result {
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
    /// Get a config value by dotted key path (e.g., "fallback.cloud_review_exhausted")
    Get {
        /// Dotted key path (e.g., "tools.codex.enabled", "review.tool")
        key: String,

        /// Default value if key not found (exit code 1 if omitted and key missing)
        #[arg(long)]
        default: Option<String>,

        /// Only search project config (skip global fallback)
        #[arg(long, conflicts_with = "global")]
        project: bool,

        /// Only search global config (skip project config)
        #[arg(long, conflicts_with = "project")]
        global: bool,

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
pub enum TiersCommands {
    /// List all configured tiers with model specs and descriptions
    List {
        /// Working directory (defaults to CWD)
        #[arg(long)]
        cd: Option<String>,
    },
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
