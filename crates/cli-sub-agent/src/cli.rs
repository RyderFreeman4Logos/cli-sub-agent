use clap::{ArgGroup, Parser, Subcommand};
use csa_core::types::{OutputFormat, ToolArg, ToolName};

#[path = "cli_todo.rs"]
mod cli_todo;
pub use cli_todo::*;

/// Build version string combining Cargo.toml version and git describe.
fn build_version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| {
        let cargo_ver = env!("CARGO_PKG_VERSION");
        let git_desc = env!("CSA_GIT_DESCRIBE");
        if git_desc.is_empty() {
            cargo_ver.to_string()
        } else {
            format!("{cargo_ver} ({git_desc})")
        }
    })
}

#[derive(Parser)]
#[command(name = "csa", version = build_version())]
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

        /// Run a named skill as a sub-agent (resolves SKILL.md + .skill.toml)
        #[arg(long)]
        skill: Option<String>,

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

        /// Bypass tier whitelist enforcement (allow any tool/model)
        #[arg(long)]
        force: bool,

        /// Disable automatic 429 failover to alternative tools
        #[arg(long)]
        no_failover: bool,

        /// Block-wait for a free slot instead of failing when all slots are occupied
        #[arg(long)]
        wait: bool,

        /// Kill child only when no streamed output appears for N seconds
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        idle_timeout: Option<u64>,

        /// Force stdout streaming to stderr even in non-TTY/non-Text contexts
        #[arg(long, conflicts_with = "no_stream_stdout")]
        stream_stdout: bool,

        /// Suppress real-time stdout streaming to stderr (streams by default for text output)
        #[arg(long)]
        no_stream_stdout: bool,
    },

    /// Manage sessions
    Session {
        #[command(subcommand)]
        cmd: SessionCommands,
    },

    /// Manage audit manifest lifecycle
    Audit {
        #[command(subcommand)]
        command: AuditCommands,
    },

    /// Initialize project configuration (.csa/config.toml)
    ///
    /// By default, creates a minimal config with only [project] metadata.
    /// Tools, tiers, and resources inherit from the global config or built-in
    /// defaults.  Use --full to auto-detect tools and generate tier configs.
    /// Use --template to write a fully-commented reference config.
    Init {
        /// Non-interactive mode
        #[arg(long)]
        non_interactive: bool,

        /// Auto-detect tools and generate full tier configuration
        #[arg(long, conflicts_with = "template")]
        full: bool,

        /// Generate a fully-commented TOML template showing all options
        #[arg(long, conflicts_with = "full")]
        template: bool,
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

    /// Execute weave workflow files
    Plan {
        #[command(subcommand)]
        cmd: PlanCommands,
    },

    /// Run pending config/state migrations
    Migrate {
        /// Show pending migrations without applying
        #[arg(long)]
        dry_run: bool,

        /// Show current vs latest version and pending migration count
        #[arg(long, conflicts_with = "dry_run")]
        status: bool,
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
#[command(group(
    ArgGroup::new("review_scope")
        .args(["diff", "commit", "range", "files"])
        .multiple(false)
))]
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
    #[arg(long, conflicts_with_all = ["diff", "commit", "range", "files"])]
    pub branch: Option<String>,

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

    /// Absolute wall-clock timeout in seconds (kills execution after N seconds when set)
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    pub timeout: Option<u64>,

    /// Kill sub-agent when no output appears for N seconds (overrides config default)
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    pub idle_timeout: Option<u64>,

    /// Force stdout streaming to stderr even in non-TTY contexts
    #[arg(long, conflicts_with = "no_stream_stdout")]
    pub stream_stdout: bool,

    /// Suppress real-time stdout streaming to stderr
    #[arg(long)]
    pub no_stream_stdout: bool,

    /// Continue without csa-review pattern (warn instead of hard error)
    #[arg(long)]
    pub allow_fallback: bool,

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

    /// Number of debate rounds (default: 3)
    #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..))]
    pub rounds: u32,

    /// Absolute wall-clock timeout in seconds (kills execution after N seconds)
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    pub timeout: Option<u64>,

    /// Kill sub-agent when no output appears for N seconds (overrides config default)
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    pub idle_timeout: Option<u64>,

    /// Force stdout streaming to stderr even in non-TTY contexts
    #[arg(long, conflicts_with = "no_stream_stdout")]
    pub stream_stdout: bool,

    /// Suppress real-time stdout streaming to stderr
    #[arg(long)]
    pub no_stream_stdout: bool,

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
}

#[derive(Subcommand)]
pub enum AuditCommands {
    /// Initialize audit manifest by scanning and hashing files
    Init {
        /// Root path to scan
        #[arg(long, default_value = ".")]
        root: String,

        /// Additional ignore patterns (prefix/path based)
        #[arg(long)]
        ignore: Vec<String>,

        /// Mirror directory for mapping source paths to output locations.
        /// When set, blog_path is auto-computed as {mirror_dir}/{source_path}.md.
        #[arg(long)]
        mirror_dir: Option<String>,
    },

    /// Show audit status by comparing manifest and current filesystem
    Status {
        /// Output format for audit status
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,

        /// Optional status filter (pending, generated, approved)
        #[arg(long)]
        filter: Option<String>,

        /// Sort order: topo (topological, leaves first), depth (deepest first), or alpha
        #[arg(long, default_value = "topo", value_parser = ["topo", "depth", "alpha"])]
        order: String,
    },

    /// Update audit metadata for files
    Update {
        /// Files to update
        #[arg(required = true)]
        files: Vec<String>,

        /// New status (pending, generated, approved)
        #[arg(long, default_value = "generated")]
        status: String,

        /// Auditor name
        #[arg(long)]
        auditor: Option<String>,

        /// Blog path associated with generated content.
        /// Overrides auto-computation from mirror_dir.
        #[arg(long)]
        blog_path: Option<String>,

        /// Mirror directory for auto-computing blog_path.
        /// Overrides manifest meta mirror_dir for this update.
        #[arg(long)]
        mirror_dir: Option<String>,
    },

    /// Mark files as approved
    Approve {
        /// Files to approve
        #[arg(required = true)]
        files: Vec<String>,

        /// Approver identity
        #[arg(long, default_value = "human")]
        approved_by: String,
    },

    /// Reset files to pending status
    Reset {
        /// Files to reset
        #[arg(required = true)]
        files: Vec<String>,
    },

    /// Reconcile manifest with filesystem state
    Sync,
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
pub enum PlanCommands {
    /// Execute a weave workflow file
    Run {
        /// Path to workflow TOML file
        file: String,

        /// Variable override (KEY=VALUE, repeatable)
        #[arg(long = "var", value_name = "KEY=VALUE")]
        vars: Vec<String>,

        /// Override tool for all CSA steps (ignores tier routing)
        #[arg(long)]
        tool: Option<ToolName>,

        /// Show execution plan without running
        #[arg(long)]
        dry_run: bool,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },
}
