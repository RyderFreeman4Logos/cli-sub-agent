use super::*;

#[derive(clap::Args)]
pub struct DebateArgs {
    /// The question or problem to debate; reads from stdin if omitted
    pub question: Option<String>,

    /// Named alias for the question (alternative to positional arg)
    #[arg(long, alias = "question", conflicts_with = "question")]
    pub topic: Option<String>,
    /// Autonomous mode flag (REQUIRED for root callers)
    #[arg(long, value_name = "BOOL")]
    pub sa_mode: Option<bool>,
    /// Tool to use for debate (overrides auto heterogeneous selection).
    /// Unlike `csa run`, explicit --tool keeps failover enabled; use --no-failover to fail fast.
    /// Combine with --tier to use that tier's model/thinking for the selected tool.
    /// Combine with --force-ignore-tier-setting to bypass tiers entirely.
    #[arg(long)]
    pub tool: Option<ToolArg>,

    /// Override tool enablement from user config (use when explicitly requesting a disabled tool)
    #[arg(long)]
    pub force_override_user_config: bool,

    /// Resume existing debate session (ULID or prefix match)
    #[arg(short, long)]
    pub session: Option<String>,

    /// Override model
    #[arg(short, long)]
    pub model: Option<String>,

    /// Exact model selector in `tool/provider/model/thinking` format.
    /// Use this for a single fixed model choice; use `--tier` for tier-managed routing and failover.
    #[arg(long, value_parser = parse_model_spec_arg)]
    pub model_spec: Option<String>,

    /// Difficulty label looked up in `[tier_mapping]` when no explicit tier/model-spec is set.
    #[arg(long, value_name = "LABEL", conflicts_with = "tier")]
    pub hint_difficulty: Option<String>,

    /// Thinking budget (low, medium, high, xhigh, max)
    #[arg(long)]
    pub thinking: Option<String>,

    /// Disable automatic retry/failover on transient errors (cross-tool 429 failover,
    /// same-tool ACP crash retry, provider rate-limit/quota retry phases, debate outer
    /// retry loop). Useful with --model-spec to pin an exact selection and fail fast.
    #[arg(long)]
    pub no_failover: bool,

    /// Enable Codex fast_mode for faster responses at higher cost. Only affects codex.
    #[arg(long)]
    pub fast_but_more_cost: bool,

    /// Override per-session memory cap/projection for this debate invocation.
    #[arg(long, value_name = "MB", value_parser = clap::value_parser!(u64).range(256..))]
    pub memory_max_mb: Option<u64>,

    /// Override minimum MemAvailable reserve for this debate invocation.
    #[arg(long, value_name = "MB")]
    pub min_free_memory_mb: Option<u64>,

    /// Number of debate rounds (default: 3)
    #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..))]
    pub rounds: u32,

    /// Validate debate plumbing without invoking the AI tool
    #[arg(long)]
    pub dry_run: bool,

    /// Exit non-zero when a completed debate emits a REVISE verdict.
    #[arg(long)]
    pub fail_on_revise: bool,

    /// Exit non-zero when a completed debate emits a REJECT verdict.
    #[arg(long)]
    pub fail_on_reject: bool,

    /// Absolute wall-clock timeout in seconds (kills execution after N seconds)
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    pub timeout: Option<u64>,

    /// Kill sub-agent when no output appears for N seconds (overrides config default)
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    pub idle_timeout: Option<u64>,

    /// Shorter timeout for first response from backend tool.
    /// Overrides config `resources.initial_response_timeout_seconds`.
    /// Set to 0 to disable.
    #[arg(long, value_parser = clap::value_parser!(u64))]
    pub initial_response_timeout: Option<u64>,

    /// Force stdout streaming to stderr even in non-TTY contexts
    #[arg(long, conflicts_with = "no_stream_stdout")]
    pub stream_stdout: bool,

    /// Suppress real-time stdout streaming to stderr
    #[arg(long)]
    pub no_stream_stdout: bool,

    /// Disable the fatal-error-marker silent-hang scan for this session. Use
    /// when debate prose legitimately contains provider error markers. The
    /// idle-timeout and wall-clock timeout still apply. Default: scan enabled
    /// (disabled by default under CSA_PATTERN_INTERNAL, i.e. for pattern-internal
    /// `csa plan run` bash steps, #1847).
    #[arg(long)]
    pub no_error_marker_scan: bool,

    /// Force-enable the fatal-error-marker scan even when CSA_PATTERN_INTERNAL
    /// would otherwise disable it (#1847). Overrides the marker-derived
    /// default; mutually exclusive with --no-error-marker-scan.
    #[arg(long, conflicts_with = "no_error_marker_scan")]
    pub error_marker_scan: bool,

    /// Working directory
    #[arg(long)]
    pub cd: Option<String>,

    /// Tier name, alias, or unambiguous prefix for tool/model routing.
    /// With --tool, resolves that tool's model/thinking from the selected tier.
    #[arg(long)]
    pub tier: Option<String>,

    /// Bypass tier routing for direct --tool/--model overrides.
    /// Use without --tier; --tool + --tier + --force-ignore-tier-setting is an error.
    #[arg(long, alias = "force-tier")]
    pub force_ignore_tier_setting: bool,

    /// Supplementary context for the debate (prepended to the question)
    #[arg(long)]
    pub context: Option<String>,

    /// Attach a file as context for the debate (content prepended to prompt)
    #[arg(long, value_name = "PATH")]
    pub file: Vec<PathBuf>,

    /// Disable filesystem sandbox isolation (bwrap/landlock)
    #[arg(long)]
    pub no_fs_sandbox: bool,

    /// Append extra writable paths to the filesystem sandbox (comma-separated)
    #[arg(long, value_delimiter = ',', value_name = "PATH")]
    pub extra_writable: Vec<PathBuf>,

    /// Expose extra host paths to the filesystem sandbox as read-only binds.
    #[arg(long, value_delimiter = ',', value_name = "PATH")]
    pub extra_readable: Vec<PathBuf>,

    /// Read the debate question from a file; use this for long multi-paragraph motions.
    /// Use `-` or `/dev/stdin` to read from stdin (heredoc or pipe).
    #[arg(long = "question-file", alias = "prompt-file", value_name = "PATH")]
    pub question_file: Option<PathBuf>,

    /// [DEPRECATED] Daemon mode is now the default. This flag is a no-op.
    #[arg(long, hide = true)]
    pub daemon: bool,

    /// Run in foreground blocking mode instead of the default daemon mode.
    #[arg(long)]
    pub no_daemon: bool,

    /// Internal flag: this process IS the daemon child. Skip re-spawning.
    #[arg(long, hide = true)]
    pub daemon_child: bool,

    /// Internal: pre-assigned session ID from daemon parent
    #[arg(long, hide = true)]
    pub session_id: Option<String>,
}
