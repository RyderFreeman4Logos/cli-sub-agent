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
    /// Tool to use for review (defaults to global [review] config or project fallback)
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

    /// Path to agent-spec file (.spec or .toml) for contract-based verification
    #[arg(long, value_name = "PATH")]
    pub spec: Option<String>,

    /// Tier name for tool/model routing (must exist in [tiers] config)
    #[arg(long)]
    pub tier: Option<String>,
}

impl ReviewArgs {
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
    if let Some(t) = timeout {
        if t < min_timeout {
            let min_minutes = min_timeout / 60;
            return Err(clap::Error::raw(
                clap::error::ErrorKind::ValueValidation,
                format!(
                    "Absolute timeout (--timeout) must be at least {min_timeout} seconds ({min_minutes} minutes). \
                     Short timeouts waste tokens because the agent starts working but gets killed before producing output. \
                     Record this in your CLAUDE.md or memory: CSA minimum timeout is {min_timeout} seconds. \
                     Configure via [execution] min_timeout_seconds in .csa/config.toml or global config."
                ),
            ));
        }
    }
    Ok(())
}

#[derive(clap::Args)]
pub struct DebateArgs {
    /// The question or problem to debate; reads from stdin if omitted
    pub question: Option<String>,
    /// Autonomous mode flag (REQUIRED for root callers)
    #[arg(long, value_name = "BOOL")]
    pub sa_mode: Option<bool>,
    /// Tool to use for debate (overrides auto heterogeneous selection)
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

    /// Thinking budget (low, medium, high, xhigh)
    #[arg(long)]
    pub thinking: Option<String>,

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

    /// Tier name for tool/model routing (must exist in [tiers] config)
    #[arg(long)]
    pub tier: Option<String>,
}
