// NOTE #1858: #[path]-included by tests; no `crate::`, no binary-only methods (dead_code).
use std::path::PathBuf;

use clap::{ArgGroup, ValueEnum};
use csa_core::types::{ToolArg, ToolName};
use serde::{Deserialize, Serialize};

use super::{Commands, parse_cli_tool_name, parse_model_spec_arg, parse_spec_path_arg};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum ReviewDepth {
    #[default]
    Standard,
    Audit,
}

impl ReviewDepth {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Audit => "audit",
        }
    }
}

impl std::fmt::Display for ReviewDepth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[value(rename_all = "kebab-case")]
#[serde(rename_all = "kebab-case")]
pub enum ReviewChunkingMode {
    Auto,
    Off,
    Always,
}

impl ReviewChunkingMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Off => "off",
            Self::Always => "always",
        }
    }
}

impl std::fmt::Display for ReviewChunkingMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(clap::Args, Clone)]
#[command(group(
    ArgGroup::new("review_scope")
        .args(["diff", "commit", "range", "files"])
        .multiple(false)
))]
#[command(group(
    ArgGroup::new("fix_finding_prompt")
        .args(["prompt", "prompt_file"])
        .multiple(false)
))]
pub struct ReviewArgs {
    /// Run the experimental observe-only convergence discovery engine.
    /// Currently requires --discovery-only and an explicit --range <base>...HEAD.
    #[arg(long)]
    pub converge: bool,

    /// Collect discovery evidence for the walking-skeleton observation cell only.
    /// This does not produce a review verdict or merge attestation.
    #[arg(long)]
    pub discovery_only: bool,

    /// Check that the current branch HEAD has a passing full-diff review verdict.
    ///
    /// This is a fast, read-only state lookup used by git hooks and PR workflows.
    #[arg(long)]
    pub check_verdict: bool,

    /// Tool to use for review (defaults to global [review] config or project fallback).
    /// Unlike `csa run`, explicit --tool keeps failover enabled; use --no-failover to fail fast.
    /// Combine with --tier to use that tier's model/thinking for the selected tool.
    /// Combine with --force-ignore-tier-setting to bypass tiers entirely.
    #[arg(long, value_parser = parse_cli_tool_name)]
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
    #[arg(long, value_parser = parse_model_spec_arg)]
    pub model_spec: Option<String>,

    /// Difficulty label looked up in `[tier_mapping]` when no explicit tier/model-spec is set.
    #[arg(long, value_name = "LABEL", conflicts_with = "tier")]
    pub hint_difficulty: Option<String>,

    /// Thinking budget (accepted for CLI compatibility but not used by review;
    /// thinking level is controlled via tier configuration)
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

    /// Override per-session memory cap/projection for this review invocation.
    #[arg(long, value_name = "MB", value_parser = clap::value_parser!(u64).range(256..))]
    pub memory_max_mb: Option<u64>,

    /// Override minimum MemAvailable reserve for this review invocation.
    #[arg(long, value_name = "MB")]
    pub min_free_memory_mb: Option<u64>,

    /// Cap build (CARGO_BUILD_JOBS) and test (NEXTEST_TEST_THREADS)
    /// parallelism of the validation gate to N. Use on memory-tight hosts
    /// to avoid OOM-kills / spawn flakes. Unset = uncapped (honors an
    /// inherited CARGO_BUILD_JOBS).
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub build_jobs: Option<u32>,

    /// Review uncommitted changes (git diff HEAD)
    #[arg(long)]
    pub diff: bool,

    /// Extend review agent consistency scan to touched files; does not change diff scope
    #[arg(long)]
    pub full_consistency: bool,

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

    /// Chunk large review diffs by module/crate before reviewer execution
    #[arg(long, value_enum, default_value_t = ReviewChunkingMode::Auto)]
    pub chunked_review: ReviewChunkingMode,

    /// Review-and-fix mode (apply fixes directly)
    #[arg(long, conflicts_with = "fix_finding")]
    pub fix: bool,

    /// Apply a caller-confirmed review finding by resuming the exact failed review session.
    #[arg(long, conflicts_with_all = ["fix", "check_verdict"])]
    pub fix_finding: bool,

    /// Maximum fix iterations when --fix is enabled (default: 3)
    #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u8).range(1..))]
    pub max_rounds: u8,

    /// Review mode: standard (default) or red-team
    #[arg(long, value_enum)]
    pub review_mode: Option<ReviewMode>,

    /// Review depth: standard (default) or audit. Audit enables red-team mode.
    #[arg(long, value_enum, default_value_t = ReviewDepth::Standard)]
    pub depth: ReviewDepth,

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

    /// Disable the fatal-error-marker silent-hang scan for this session. Use
    /// ONLY when developing CSA's own error/quota/failover detection code,
    /// whose source and test fixtures legitimately contain provider error
    /// markers (#1745). The idle-timeout and wall-clock timeout still apply.
    /// Default: scan enabled (disabled by default under CSA_PATTERN_INTERNAL,
    /// i.e. for pattern-internal `csa plan run` bash steps, #1847).
    #[arg(long)]
    pub no_error_marker_scan: bool,

    /// Force-enable the fatal-error-marker scan even when CSA_PATTERN_INTERNAL
    /// would otherwise disable it (#1847). Overrides the marker-derived
    /// default; mutually exclusive with --no-error-marker-scan.
    #[arg(long, conflicts_with = "no_error_marker_scan")]
    pub error_marker_scan: bool,

    /// Continue without csa-review pattern (warn instead of hard error)
    #[arg(long)]
    pub allow_fallback: bool,

    /// Working directory
    #[arg(long)]
    pub cd: Option<String>,

    /// Path to agent-spec file (.spec or .toml) for contract-based verification
    #[arg(long, value_name = "PATH", value_parser = parse_spec_path_arg)]
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

    /// Enables D-Bus user bus and systemd private socket access inside the sandbox.
    /// Use for verification sessions that need to restart user daemons. All usage is audit-logged.
    #[arg(long)]
    pub allow_user_daemon_ipc: bool,

    /// Append extra writable paths to the filesystem sandbox (comma-separated)
    #[arg(long, value_delimiter = ',', value_name = "PATH")]
    pub extra_writable: Vec<PathBuf>,

    /// Expose extra host paths to the filesystem sandbox as read-only binds.
    #[arg(long, value_delimiter = ',', value_name = "PATH")]
    pub extra_readable: Vec<PathBuf>,

    /// Caller-confirmed fix prompt text for --fix-finding.
    #[arg(long, value_name = "PROMPT", conflicts_with = "prompt_file")]
    pub prompt: Option<String>,

    /// Read supplementary prompt/context from a file path (bypasses shell quoting issues).
    /// For --fix-finding, use `-` or `/dev/stdin` to read the caller-confirmed prompt from stdin.
    /// Regular review treats this as a path; stdin sentinels are not resolved.
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
        self.effective_security_mode_for(self.effective_review_mode())
    }

    pub fn effective_security_mode_for(&self, review_mode: ReviewMode) -> &str {
        if review_mode == ReviewMode::RedTeam && self.security_mode == "auto" {
            "on"
        } else {
            &self.security_mode
        }
    }
}

pub fn validate_review_args(args: &ReviewArgs) -> std::result::Result<(), clap::Error> {
    validate_convergence_args(args)?;
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

    if args.depth == ReviewDepth::Audit && args.security_mode == "off" {
        return Err(clap::Error::raw(
            clap::error::ErrorKind::ArgumentConflict,
            "--depth audit conflicts with --security-mode off",
        ));
    }

    if args.single && args.requested_reviewers() > 1 {
        return Err(clap::Error::raw(
            clap::error::ErrorKind::ArgumentConflict,
            "--single conflicts with --reviewers > 1",
        ));
    }

    if args.fix_finding && args.session.is_none() {
        return Err(clap::Error::raw(
            clap::error::ErrorKind::MissingRequiredArgument,
            "--fix-finding requires --session <failed-review-session-id>",
        ));
    }

    if !args.fix_finding && args.prompt.is_some() {
        return Err(clap::Error::raw(
            clap::error::ErrorKind::ArgumentConflict,
            "--prompt is only valid with --fix-finding",
        ));
    }

    if args.fix_finding && args.requested_reviewers() > 1 {
        return Err(clap::Error::raw(
            clap::error::ErrorKind::ArgumentConflict,
            "--fix-finding resumes one exact reviewer session and conflicts with --reviewers > 1",
        ));
    }

    Ok(())
}

fn validate_convergence_args(args: &ReviewArgs) -> std::result::Result<(), clap::Error> {
    if !args.converge && !args.discovery_only {
        return Ok(());
    }

    let error = |kind, detail: &str| {
        clap::Error::raw(
            kind,
            format!(
                "experimental observe-only convergence discovery: {detail}; this walking skeleton never falls back to ordinary review"
            ),
        )
    };
    if args.converge != args.discovery_only {
        return Err(error(
            clap::error::ErrorKind::MissingRequiredArgument,
            "--converge and --discovery-only currently require each other",
        ));
    }
    let Some(range) = args.range.as_deref() else {
        return Err(error(
            clap::error::ErrorKind::MissingRequiredArgument,
            "an explicit --range <base>...HEAD is required",
        ));
    };
    let Some(base) = range.strip_suffix("...HEAD") else {
        return Err(error(
            clap::error::ErrorKind::ValueValidation,
            "--range must use the exact three-dot form <base>...HEAD",
        ));
    };
    if base.is_empty() || base.contains("..") {
        return Err(error(
            clap::error::ErrorKind::ValueValidation,
            "--range must name a nonempty base in the exact form <base>...HEAD",
        ));
    }

    let conflict = if args.check_verdict {
        Some("--check-verdict")
    } else if args.fix {
        Some("--fix")
    } else if args.fix_finding {
        Some("--fix-finding")
    } else if args.session.is_some() {
        Some("--session/--resume")
    } else if args.diff {
        Some("--diff")
    } else if args.branch.is_some() {
        Some("--branch")
    } else if args.commit.is_some() {
        Some("--commit")
    } else if args.files.is_some() {
        Some("--files")
    } else if args.requested_reviewers() > 1 {
        Some("--reviewers > 1")
    } else if args.context.is_some() {
        Some("--context")
    } else if args.prompt.is_some() {
        Some("--prompt")
    } else if args.prompt_file.is_some() {
        Some("--prompt-file")
    } else if args.spec.is_some() {
        Some("--spec")
    } else if args.no_fs_sandbox {
        Some("--no-fs-sandbox")
    } else if args.allow_user_daemon_ipc {
        Some("--allow-user-daemon-ipc")
    } else if !args.extra_writable.is_empty() {
        Some("--extra-writable")
    } else if !args.extra_readable.is_empty() {
        Some("--extra-readable")
    } else if args.prior_rounds_summary.is_some() {
        Some("--prior-rounds-summary")
    } else {
        None
    };
    if let Some(flag) = conflict {
        return Err(error(
            clap::error::ErrorKind::ArgumentConflict,
            &format!("{flag} is outside this immutable discovery-only slice"),
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
        Commands::Hunt(args) => {
            validate_timeout(Some(args.timeout), min_timeout)?;
        }
        Commands::Arch(args) => {
            validate_timeout(Some(args.timeout), min_timeout)?;
        }
        Commands::Triage(args) => {
            validate_timeout(Some(args.timeout), min_timeout)?;
        }
        Commands::Mktsk(args) => {
            validate_timeout(Some(args.timeout), min_timeout)?;
        }
        Commands::Review(args) => {
            validate_review_args(args)?;
            if !args.check_verdict {
                validate_timeout(args.timeout, min_timeout)?;
            }
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
            timeout_validation_message(t, min_timeout),
        ));
    }
    Ok(())
}

fn timeout_validation_message(given: u64, min_timeout: u64) -> String {
    let min_minutes = min_timeout / 60;
    let suggested = build_suggested_command(given, min_timeout);
    format!(
        "Absolute timeout (--timeout) must be at least {min_timeout} seconds ({min_minutes} minutes). \
         Short timeouts waste tokens because the agent starts working but gets killed before producing output. \
         Record this in your CLAUDE.md or memory: CSA minimum timeout is {min_timeout} seconds. \
         Use --timeout >= {min_timeout}, or inspect the effective floor with `csa config get execution.min_timeout_seconds`.\n\
         Suggested: {suggested}"
    )
}

fn build_suggested_command(given: u64, min_timeout: u64) -> String {
    let args: Vec<String> = std::env::args().collect();
    let given_str = given.to_string();
    let min_str = min_timeout.to_string();

    let mut result = args.clone();
    let mut replaced = false;
    let mut i = 0;

    while i < result.len() {
        if result[i] == "--timeout" && i + 1 < result.len() && result[i + 1] == given_str {
            result[i + 1] = min_str.clone();
            replaced = true;
            break;
        }
        if result[i] == format!("--timeout={given_str}") {
            result[i] = format!("--timeout={min_str}");
            replaced = true;
            break;
        }
        i += 1;
    }

    if replaced {
        result.join(" ")
    } else {
        format!("csa ... --timeout {min_str}")
    }
}

#[path = "cli_debate.rs"]
mod cli_debate;
pub use cli_debate::DebateArgs;

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
        assert!(
            rendered.contains("Suggested:"),
            "error should include a ready-to-copy corrected command"
        );
        assert!(
            rendered.contains("--timeout 1800"),
            "suggestion should show the corrected floor timeout"
        );
    }

    #[test]
    fn timeout_validation_message_guides_toward_effective_floor() {
        let rendered = timeout_validation_message(1200, 2400);
        assert!(rendered.contains("Use --timeout >= 2400"));
        assert!(rendered.contains("effective floor"));
        assert!(rendered.contains("Suggested:"));
        assert!(rendered.contains("--timeout 2400"));
    }
}
