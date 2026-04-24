use std::path::PathBuf;

use clap::{ArgGroup, ValueEnum};
use csa_core::types::ToolName;

use super::Commands;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum ReviewMode {
    Standard,
    RedTeam,
}

impl ReviewMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::RedTeam => "red-team",
        }
    }
}

impl std::fmt::Display for ReviewMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(clap::Args)]
#[command(group(
    ArgGroup::new("review_scope")
        .args(["diff", "commit", "range", "files"])
        .multiple(false)
))]
pub struct ReviewArgs {
    /// Tool to use for review (defaults to global [review] config or project fallback).
    /// Combine with --tier to use that tier's model/thinking for the selected tool.
    /// Combine with --force-ignore-tier-setting to bypass tiers entirely.
    #[arg(long)]
    pub tool: Option<ToolName>,
    /// Autonomous mode flag (REQUIRED for root callers)
    #[arg(long, value_name = "BOOL")]
    pub sa_mode: Option<bool>,
    /// Override tool enablement from user config (use when explicitly requesting a disabled tool)
    #[arg(long)]
    pub force_override_user_config: bool,

    /// Resume existing review session
    #[arg(short, long)]
    pub session: Option<String>,

    /// Override model
    #[arg(short, long)]
    pub model: Option<String>,

    /// Exact model selector in `tool/provider/model/thinking` format.
    /// Use this for a single fixed model choice; use `--tier` for tier-managed routing and failover.
    #[arg(long)]
    pub model_spec: Option<String>,

    /// Difficulty label looked up in `[tier_mapping]` when no explicit tier/model-spec is set.
    #[arg(long, value_name = "LABEL")]
    pub hint_difficulty: Option<String>,

    /// Thinking budget (accepted for CLI compatibility but not used by review;
    /// thinking level is controlled via tier configuration)
    #[arg(long)]
    pub thinking: Option<String>,

    /// Disable automatic retry/failover on transient errors (cross-tool 429 failover,
    /// same-tool ACP crash retry, Gemini rate-limit/quota retry phases, debate outer
    /// retry loop). Useful with --model-spec to pin an exact selection and fail fast.
    #[arg(long)]
    pub no_failover: bool,

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

    /// Maximum fix iterations when --fix is enabled (default: 3)
    #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u8).range(1..))]
    pub max_rounds: u8,

    /// Review mode: standard (default) or red-team
    #[arg(long, value_enum)]
    pub review_mode: Option<ReviewMode>,

    /// Shorthand for `--review-mode red-team`
    #[arg(long)]
    pub red_team: bool,

    /// Security review mode: auto, on, off
    #[arg(long, default_value = "auto", value_parser = ["auto", "on", "off"])]
    pub security_mode: String,

    /// Path to context file (e.g., TODO plan)
    #[arg(long)]
    pub context: Option<String>,

    /// Number of reviewers to run in parallel (default: 1).
    /// `--range` auto-selects up to 3 heterogeneous reviewers from a multi-tool tier
    /// unless `--reviewers`, `--single`, `--tool`, or `--model-spec` overrides it.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    pub reviewers: Option<u32>,

    /// Force single-reviewer mode even when `--range` would auto-select heterogeneous reviewers.
    #[arg(long)]
    pub single: bool,

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

    /// Continue without csa-review pattern (warn instead of hard error)
    #[arg(long)]
    pub allow_fallback: bool,

    /// Working directory
    #[arg(long)]
    pub cd: Option<String>,

    /// Path to agent-spec file (.spec or .toml) for contract-based verification
    #[arg(long, value_name = "PATH")]
    pub spec: Option<String>,

    /// Tier name, alias, or unambiguous prefix for tool/model routing.
    /// With --tool, resolves that tool's model/thinking from the selected tier.
    #[arg(long)]
    pub tier: Option<String>,

    /// Bypass tier routing for direct --tool/--model overrides.
    /// Use without --tier; --tool + --tier + --force-ignore-tier-setting is an error.
    #[arg(long, alias = "force-tier")]
    pub force_ignore_tier_setting: bool,

    /// Disable filesystem sandbox isolation (bwrap/landlock)
    #[arg(long)]
    pub no_fs_sandbox: bool,

    /// Append extra writable paths to the filesystem sandbox (comma-separated)
    #[arg(long, value_delimiter = ',', value_name = "PATH")]
    pub extra_writable: Vec<PathBuf>,

    /// Expose extra host paths to the filesystem sandbox as read-only binds.
    #[arg(long, value_delimiter = ',', value_name = "PATH")]
    pub extra_readable: Vec<PathBuf>,

    /// Read supplementary prompt/context from a file (bypasses shell quoting issues)
    #[arg(long, value_name = "PATH")]
    pub prompt_file: Option<PathBuf>,

    /// TOML file containing prior-round fix summaries and invariants to re-verify
    #[arg(long, value_name = "PATH")]
    pub prior_rounds_summary: Option<PathBuf>,

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

impl ReviewArgs {
    pub fn requested_reviewers(&self) -> u32 {
        self.reviewers.unwrap_or(1)
    }

    pub fn effective_review_mode(&self) -> ReviewMode {
        if self.red_team {
            ReviewMode::RedTeam
        } else {
            self.review_mode.unwrap_or(ReviewMode::Standard)
        }
    }

    pub fn effective_security_mode(&self) -> &str {
        if self.effective_review_mode() == ReviewMode::RedTeam && self.security_mode == "auto" {
            "on"
        } else {
            &self.security_mode
        }
    }
}

pub fn validate_review_args(args: &ReviewArgs) -> std::result::Result<(), clap::Error> {
    let effective_review_mode = args.effective_review_mode();

    if args.red_team && matches!(args.review_mode, Some(ReviewMode::Standard)) {
        return Err(clap::Error::raw(
            clap::error::ErrorKind::ArgumentConflict,
            "--red-team conflicts with --review-mode standard",
        ));
    }

    if effective_review_mode == ReviewMode::RedTeam && args.security_mode == "off" {
        let mode_flag = if args.red_team {
            "--red-team"
        } else {
            "--review-mode red-team"
        };

        return Err(clap::Error::raw(
            clap::error::ErrorKind::ArgumentConflict,
            format!("{mode_flag} conflicts with --security-mode off"),
        ));
    }

    if args.single && args.requested_reviewers() > 1 {
        return Err(clap::Error::raw(
            clap::error::ErrorKind::ArgumentConflict,
            "--single conflicts with --reviewers > 1",
        ));
    }

    Ok(())
}

pub fn validate_command_args(
    command: &Commands,
    min_timeout: u64,
) -> std::result::Result<(), clap::Error> {
    match command {
        Commands::Run { timeout, .. } => {
            validate_timeout(*timeout, min_timeout)?;
        }
        Commands::Review(args) => {
            validate_review_args(args)?;
            validate_timeout(args.timeout, min_timeout)?;
        }
        Commands::Debate(args) => {
            validate_timeout(args.timeout, min_timeout)?;
        }
        _ => {}
    }

    Ok(())
}

fn validate_timeout(
    timeout: Option<u64>,
    min_timeout: u64,
) -> std::result::Result<(), clap::Error> {
    if let Some(t) = timeout
        && t < min_timeout
    {
        return Err(clap::Error::raw(
            clap::error::ErrorKind::ValueValidation,
            timeout_validation_message(min_timeout),
        ));
    }
    Ok(())
}

fn timeout_validation_message(min_timeout: u64) -> String {
    let min_minutes = min_timeout / 60;
    format!(
        "Absolute timeout (--timeout) must be at least {min_timeout} seconds ({min_minutes} minutes). \
         Short timeouts waste tokens because the agent starts working but gets killed before producing output. \
         Record this in your CLAUDE.md or memory: CSA minimum timeout is {min_timeout} seconds. \
         Use --timeout >= {min_timeout}, or inspect the effective floor with `csa config get execution.min_timeout_seconds`."
    )
}

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
    /// Combine with --tier to use that tier's model/thinking for the selected tool.
    /// Combine with --force-ignore-tier-setting to bypass tiers entirely.
    #[arg(long)]
    pub tool: Option<ToolName>,

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
    #[arg(long)]
    pub model_spec: Option<String>,

    /// Difficulty label looked up in `[tier_mapping]` when no explicit tier/model-spec is set.
    #[arg(long, value_name = "LABEL")]
    pub hint_difficulty: Option<String>,

    /// Thinking budget (low, medium, high, xhigh, max)
    #[arg(long)]
    pub thinking: Option<String>,

    /// Disable automatic retry/failover on transient errors (cross-tool 429 failover,
    /// same-tool ACP crash retry, Gemini rate-limit/quota retry phases, debate outer
    /// retry loop). Useful with --model-spec to pin an exact selection and fail fast.
    #[arg(long)]
    pub no_failover: bool,

    /// Number of debate rounds (default: 3)
    #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..))]
    pub rounds: u32,

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
    #[arg(long)]
    pub file: Option<String>,

    /// Disable filesystem sandbox isolation (bwrap/landlock)
    #[arg(long)]
    pub no_fs_sandbox: bool,

    /// Append extra writable paths to the filesystem sandbox (comma-separated)
    #[arg(long, value_delimiter = ',', value_name = "PATH")]
    pub extra_writable: Vec<PathBuf>,

    /// Expose extra host paths to the filesystem sandbox as read-only binds.
    #[arg(long, value_delimiter = ',', value_name = "PATH")]
    pub extra_readable: Vec<PathBuf>,

    /// Read the debate question from a file (bypasses shell quoting issues)
    #[arg(long, value_name = "PATH", conflicts_with_all = ["question", "topic"])]
    pub prompt_file: Option<PathBuf>,

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

#[cfg(test)]
mod tests {
    use super::{timeout_validation_message, validate_timeout};

    #[test]
    fn validate_timeout_rejects_sub_floor_without_lowering_hint() {
        let err = validate_timeout(Some(600), 1800).expect_err("timeout below floor must fail");
        let rendered = err.to_string();
        assert!(rendered.contains("must be at least 1800 seconds"));
        assert!(rendered.contains("csa config get execution.min_timeout_seconds"));
        assert!(
            !rendered.contains("Configure via [execution] min_timeout_seconds"),
            "error text should not encourage lowering the configured safety floor"
        );
    }

    #[test]
    fn timeout_validation_message_guides_toward_effective_floor() {
        let rendered = timeout_validation_message(2400);
        assert!(rendered.contains("Use --timeout >= 2400"));
        assert!(rendered.contains("effective floor"));
    }
}
