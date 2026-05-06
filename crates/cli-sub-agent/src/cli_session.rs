//! CLI subcommands for the `csa session` command group.

use std::path::PathBuf;

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

        /// Show tree structure (incompatible with --limit/--since/--status)
        #[arg(long)]
        tree: bool,

        /// List sessions from all projects (incompatible with --tree)
        #[arg(long, conflicts_with = "tree")]
        all_projects: bool,

        /// Show only the N most recent sessions
        #[arg(long)]
        limit: Option<usize>,

        /// Show sessions accessed since a duration ago (e.g., "1h", "30m", "2d")
        #[arg(long)]
        since: Option<String>,

        /// Filter by session status (active, retired, failed, error)
        #[arg(long)]
        status: Option<String>,

        /// Filter by recorded CSA binary version (exact string match)
        #[arg(long = "csa-version")]
        csa_version: Option<String>,

        /// Show the recorded CSA binary version column in text output
        #[arg(long = "show-version")]
        show_version: bool,
    },

    /// Compress session context (gemini-cli: /compress, others: /compact)
    Compress {
        /// Session ULID or prefix (positional alternative to --session)
        #[arg(conflicts_with = "session")]
        session_id: Option<String>,

        #[arg(short, long)]
        session: Option<String>,

        /// Working directory (defaults to CWD)
        #[arg(long)]
        cd: Option<String>,
    },

    /// Delete a session
    Delete {
        /// Session ULID or prefix (positional alternative to --session)
        #[arg(conflicts_with = "session")]
        session_id: Option<String>,

        #[arg(short, long)]
        session: Option<String>,

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
        /// Session ULID or prefix (positional alternative to --session)
        #[arg(conflicts_with = "session")]
        session_id: Option<String>,

        /// Session ULID or prefix
        #[arg(short, long)]
        session: Option<String>,

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
        /// Session ID or prefix (positional alternative to --session)
        #[arg(conflicts_with = "session")]
        session_id: Option<String>,

        /// Session ID or prefix
        #[arg(short, long)]
        session: Option<String>,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// Show the last execution result for a session
    Result {
        /// Session ID or prefix (positional alternative to --session)
        #[arg(conflicts_with = "session")]
        session_id: Option<String>,

        /// Session ID or prefix
        #[arg(short, long)]
        session: Option<String>,

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
        /// Session ID or prefix (positional alternative to --session)
        #[arg(conflicts_with = "session")]
        session_id: Option<String>,

        /// Session ID or prefix
        #[arg(short, long)]
        session: Option<String>,

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

    /// Show checkpoints recorded for a session
    Checkpoint {
        /// Session ULID or prefix (positional alternative to --session)
        #[arg(conflicts_with = "session")]
        session_id: Option<String>,

        /// Session ULID or prefix
        #[arg(long)]
        session: Option<String>,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,

        /// Show all checkpoints, not just the latest
        #[arg(long)]
        all: bool,
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
    /// Timeout comes from `~/.config/cli-sub-agent/config.toml` `[kv_cache].long_poll_seconds`
    /// with a legacy 250s fallback when `[kv_cache]` is absent.
    ///
    /// Optional memory early-exit: `--memory-warn-mb <N>` (or config
    /// `[session_wait].memory_warn_mb`) samples the watched session's process-tree RSS
    /// every 15s while waiting. When RSS exceeds the threshold, this command prints:
    ///
    /// `<!-- CSA:MEMORY_WARN session=<ULID> rss_mb=<N> limit_mb=<M> -->`
    ///
    /// to stdout and exits with code 33, without killing the session or emitting a
    /// completion notification.
    Wait {
        /// Session ID to wait for (positional alternative to --session)
        #[arg(conflicts_with = "session")]
        session_id: Option<String>,

        /// Session ID to wait for
        #[arg(long)]
        session: Option<String>,

        /// Override `[session_wait].memory_warn_mb` for this invocation.
        /// `0` disables the sampler for this wait command.
        #[arg(long)]
        memory_warn_mb: Option<u64>,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// Kill a running daemon session (SIGTERM, then SIGKILL after grace period)
    Kill {
        /// Session ID to kill (positional alternative to --session)
        #[arg(conflicts_with = "session")]
        session_id: Option<String>,

        /// Session ID to kill
        #[arg(long)]
        session: Option<String>,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },

    /// Attach to a running daemon session (tail live output/stderr until the daemon exits)
    Attach {
        /// Session ID to attach to (positional alternative to --session)
        #[arg(conflicts_with = "session")]
        session_id: Option<String>,

        /// Session ID to attach to
        #[arg(long)]
        session: Option<String>,

        /// Continuation prompt
        prompt: Option<String>,

        /// Continuation prompt (flag form; same as the positional prompt)
        #[arg(long = "prompt", value_name = "PROMPT", conflicts_with_all = ["prompt", "prompt_file"])]
        prompt_flag: Option<String>,

        /// Read continuation prompt from a file. Use `-` to read from stdin.
        #[arg(long, value_name = "PATH", conflicts_with = "prompt")]
        prompt_file: Option<PathBuf>,

        /// Show stderr alongside stdout
        #[arg(long)]
        stderr: bool,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,
    },
}
