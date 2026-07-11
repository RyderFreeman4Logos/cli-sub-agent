use super::*;

#[derive(Subcommand)]
pub enum Commands {
    /// Execute a task using a specific AI tool
    Run {
        /// Tool selection: auto, any-available, or a specific tool.
        #[arg(long)]
        tool: Option<ToolArg>,
        /// Auto-route via `[tier_mapping]` while keeping tool choice automatic.
        #[arg(long, value_name = "INTENT", conflicts_with = "tier")]
        auto_route: Option<String>,
        /// Difficulty label looked up in `[tier_mapping]` when no explicit tier/model-spec is set.
        #[arg(long, value_name = "LABEL", conflicts_with_all = ["tier", "auto_route"])]
        hint_difficulty: Option<String>,
        /// Run a named skill as a sub-agent (resolves SKILL.md + .skill.toml)
        #[arg(long)]
        skill: Option<String>,
        /// Autonomous mode flag for prompt-guard safety.
        #[arg(long, value_name = "BOOL")]
        sa_mode: Option<bool>,
        /// Task prompt; reads from stdin if omitted
        prompt: Option<String>,
        /// Autonomous goal mode: loop until success criteria met or budget exhausted
        #[arg(long)]
        goal: Option<String>,
        /// Task prompt (flag form; same as the positional prompt)
        #[arg(long = "prompt", value_name = "PROMPT", conflicts_with_all = ["prompt", "prompt_file"])]
        prompt_flag: Option<String>,
        /// Read prompt from a file; use `-` or `/dev/stdin` for stdin.
        #[arg(long, value_name = "PATH", conflicts_with = "prompt")]
        prompt_file: Option<PathBuf>,
        /// Add prior review context to the prompt
        #[arg(long, value_name = "SESSION")]
        inline_context_from_review_session: Option<String>,
        /// Resume existing session (ULID or prefix match) [DEPRECATED: use --fork-from]
        #[arg(short, long, conflicts_with_all = ["last", "fork_from", "fork_last", "fork_from_caller"])]
        session: Option<String>,
        /// Resume the most recent session for this project [DEPRECATED: use --fork-last]
        #[arg(long, conflicts_with_all = ["session", "ephemeral", "fork_from", "fork_last", "fork_from_caller"])]
        last: bool,
        /// Fork from a specific session (ULID or prefix match)
        #[arg(long, conflicts_with_all = ["session", "last", "fork_last", "fork_from_caller", "ephemeral"])]
        fork_from: Option<String>,
        /// Fork the most recent session for this project
        #[arg(long, conflicts_with_all = ["session", "last", "fork_from", "fork_from_caller", "ephemeral"])]
        fork_last: bool,
        /// Fork from the auto-detected caller's Claude conversation.
        #[arg(long, conflicts_with_all = ["session", "last", "fork_from", "fork_last", "ephemeral"])]
        fork_from_caller: bool,
        /// Human-readable description for a new session
        #[arg(short, long)]
        description: Option<String>,
        /// Fork-call mode: fork a session and return only the ReturnPacket.
        #[arg(long, conflicts_with_all = ["session", "last", "ephemeral"])]
        fork_call: bool,
        /// Session to return results to after fork-call completion.
        #[arg(
            long,
            requires = "fork_call",
            value_name = "TARGET",
            value_parser = validate_return_to
        )]
        return_to: Option<String>,
        /// Parent session ULID (defaults to CSA_SESSION_ID env var)
        #[arg(long, hide = true)]
        parent: Option<String>,
        /// Ephemeral session (no session persistence, auto-cleanup; tool runs in project dir)
        #[arg(long, conflicts_with = "session")]
        ephemeral: bool,
        /// Allow working on the base branch (main/dev).
        #[arg(long, alias = "allow-base-branch-commit")]
        allow_base_branch_working: bool,
        /// Working directory (defaults to CWD)
        #[arg(long)]
        cd: Option<String>,
        /// Exact `tool/provider/model/thinking` selector.
        #[arg(long, value_parser = parse_model_spec_arg)]
        model_spec: Option<String>,
        /// Override tool default model (opaque string, passed through to tool)
        #[arg(short, long)]
        model: Option<String>,

        /// Thinking budget (low, medium, high, xhigh, max)
        #[arg(long)]
        thinking: Option<String>,

        /// Bypass tier whitelist enforcement (allow any tool/model)
        #[arg(long)]
        force: bool,

        /// Override disabled-tool config for explicit requests
        #[arg(long)]
        force_override_user_config: bool,

        /// Allow tier failover with explicit --tool
        #[arg(long)]
        allow_fallback: bool,

        /// Disable retry/failover paths.
        #[arg(long)]
        no_failover: bool,

        /// Enable Codex fast_mode
        #[arg(long)]
        fast_but_more_cost: bool,

        /// Cap validation jobs. Unset honors inherited env.
        #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
        build_jobs: Option<u32>,

        /// Per-run memory cap/projection override
        #[arg(long, value_name = "MB", value_parser = clap::value_parser!(u64).range(256..))]
        memory_max_mb: Option<u64>,

        /// Per-run MemAvailable reserve override
        #[arg(long, value_name = "MB")]
        min_free_memory_mb: Option<u64>,

        /// Block-wait for a free slot instead of failing when all slots are occupied
        #[arg(long)]
        wait: bool,

        /// Kill child only when no streamed output appears for N seconds
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        idle_timeout: Option<u64>,

        /// Override first-response timeout; set 0 to disable.
        #[arg(long, value_parser = clap::value_parser!(u64))]
        initial_response_timeout: Option<u64>,

        /// Absolute wall-clock timeout in seconds (kills execution after N seconds)
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        timeout: Option<u64>,

        /// Disable idle-timeout killing (run until process exits or wall-clock timeout fires)
        #[arg(long, conflicts_with = "idle_timeout")]
        no_idle_timeout: bool,

        /// Disable memory injection for this run (overrides memory.inject=true config)
        #[arg(long)]
        no_memory: bool,

        /// Override memory search query used for prompt injection
        #[arg(long)]
        memory_query: Option<String>,

        /// Force stdout streaming to stderr even in non-TTY/non-Text contexts
        #[arg(long, conflicts_with = "no_stream_stdout")]
        stream_stdout: bool,

        /// Suppress real-time stdout streaming to stderr (streams by default for text output)
        #[arg(long)]
        no_stream_stdout: bool,

        /// Disable provider fatal-marker scan (#1652/#1745); default on except CSA_PATTERN_INTERNAL.
        #[arg(long)]
        no_error_marker_scan: bool,

        /// Force-enable the fatal-marker scan.
        #[arg(long, conflicts_with = "no_error_marker_scan")]
        error_marker_scan: bool,

        /// Disable hook-bypass scanning. Default: enabled.
        #[arg(long)]
        no_hook_bypass_scan: bool,

        /// Skip the AI-config symlink preflight for this run only.
        #[arg(long)]
        no_preflight: bool,

        /// Skip only the post-exec shell gate; external verification still applies.
        #[arg(long)]
        no_post_exec_gate: bool,

        /// Fail if a non-SA writer run ends with uncommitted worktree changes.
        #[arg(long)]
        require_commit: bool,

        /// Permit git push.
        #[arg(long)]
        allow_git_push: bool,

        /// Path to agent-spec file (.spec or .toml) for contract-based verification
        #[arg(long, value_name = "PATH", value_parser = parse_spec_path_arg)]
        spec: Option<String>,

        /// Tier name, alias, or unambiguous prefix for tool/model routing.
        /// With --tool, resolves that tool's model/thinking from the selected tier.
        #[arg(long)]
        tier: Option<String>,

        /// Bypass tier routing for direct --tool/--model overrides; invalid with `--tier`.
        #[arg(long, alias = "force-tier")]
        force_ignore_tier_setting: bool,

        /// Disable filesystem sandbox isolation (bwrap/landlock)
        #[arg(long)]
        no_fs_sandbox: bool,

        /// Enables D-Bus user bus and systemd private socket access inside the sandbox.
        /// Use for verification sessions that need to restart user daemons. All usage is audit-logged.
        #[arg(long)]
        allow_user_daemon_ipc: bool,

        /// Append extra writable paths to the filesystem sandbox (comma-separated)
        #[arg(long, value_delimiter = ',', value_name = "PATH")]
        extra_writable: Vec<PathBuf>,

        /// Expose extra host paths to the filesystem sandbox as read-only binds.
        #[arg(long = "extra-readable", value_delimiter = ',', value_name = "PATH")]
        extra_readable: Vec<PathBuf>,
        /// Deprecated no-op; daemon mode is the default.
        #[arg(long, hide = true)]
        daemon: bool,

        /// Run in foreground blocking mode instead of the default daemon mode.
        #[arg(long)]
        no_daemon: bool,

        /// Internal daemon-child marker.
        #[arg(long, hide = true)]
        daemon_child: bool,

        /// Internal session ID from daemon parent.
        #[arg(long, hide = true)]
        session_id: Option<String>,
    },

    /// Start a root-cause-first diagnostic debugging session
    Hunt(HuntArgs),

    /// Run deep module architecture analysis
    Arch(ArchArgs),

    /// Triage a GitHub issue into category and state roles
    Triage(TriageArgs),

    /// Decompose a TODO plan into compact-resilient task entries
    Mktsk(MktskArgs),

    /// Manage sessions
    Session {
        #[command(subcommand)]
        cmd: SessionCommands,
    },

    /// Push the current branch only after a passing review covers HEAD
    Push(PushArgs),

    /// Merge a GitHub pull request and sync the base branch
    Merge(MergeArgs),

    /// Manage audit manifest lifecycle
    Audit {
        #[command(subcommand)]
        command: AuditCommands,
    },

    /// Initialize project configuration (.csa/config.toml)
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

    /// Garbage collect stale session artifacts
    Gc(super::super::gc::GcArgs),

    /// Show/manage configuration
    Config {
        #[command(subcommand)]
        cmd: ConfigCommands,
    },

    /// Manage cross-session memory
    Memory {
        #[command(subcommand)]
        command: MemoryCommands,
    },

    /// Review code changes using an AI tool
    Review(ReviewArgs),

    /// Run an adversarial debate between heterogeneous AI tools
    Debate(DebateArgs),

    /// Check environment and tool availability
    Doctor {
        #[command(subcommand)]
        subcommand: Option<DoctorSubcommand>,
    },

    /// Execute tasks from a batch file
    Batch {
        /// Path to batch TOML file
        file: String,
        /// Autonomous mode flag (REQUIRED for root callers)
        #[arg(long, value_name = "BOOL")]
        sa_mode: Option<bool>,
        /// Working directory
        #[arg(long)]
        cd: Option<String>,

        /// Show what would be executed without running
        #[arg(long)]
        dry_run: bool,
    },

    /// Run as MCP server (JSON-RPC over stdio)
    McpServer,

    /// Manage shared MCP Hub daemon
    McpHub {
        #[command(subcommand)]
        cmd: McpHubCommands,
    },

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

    /// Review checklist management
    Checklist {
        #[command(subcommand)]
        command: ChecklistCommands,
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

    /// Evaluate session history (passive analysis)
    Eval {
        /// Run in passive mode (read-only analysis, no modifications)
        #[arg(long)]
        passive: bool,

        /// Project storage key (defaults to current project)
        #[arg(long)]
        project: Option<String>,

        /// Number of days to look back (default: 7)
        #[arg(long, default_value = "7")]
        days: u32,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Token estimation for files
    Tokuin {
        #[command(subcommand)]
        cmd: TokuinCommands,
    },

    /// Verify a falsifiable claim against baseline and treatment git refs
    Verify(VerifyArgs),

    /// Analyze workspace token health
    Health(HealthArgs),

    /// Query AI tool conversation threads via xurl
    Xurl {
        #[command(subcommand)]
        cmd: XurlCommands,
    },

    /// Recover main-agent context from recorded session history
    Recall(RecallArgs),

    /// Manage CSA hooks
    Hooks {
        #[command(subcommand)]
        cmd: HooksCommands,
    },
}
