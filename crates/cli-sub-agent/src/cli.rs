// NOTE #1858: #[path]-included by tests; no `crate::`, no binary-only methods (dead_code).
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use csa_core::types::{OutputFormat, ToolArg};

#[path = "cli_common.rs"]
mod cli_common;
pub use cli_common::*;
#[path = "cli_session.rs"]
mod cli_session;
pub use cli_session::*;
#[path = "cli_arch.rs"]
mod cli_arch;
pub use cli_arch::*;
#[path = "cli_todo.rs"]
mod cli_todo;
pub use cli_todo::*;
#[path = "cli_doctor.rs"]
mod cli_doctor;
pub use cli_doctor::*;
#[path = "cli_review.rs"]
mod cli_review;
pub use cli_review::*;
#[path = "cli_tokuin.rs"]
mod cli_tokuin;
pub use cli_tokuin::*;
#[path = "cli_health.rs"]
mod cli_health;
pub use cli_health::*;
#[path = "cli_xurl.rs"]
mod cli_xurl;
pub use cli_xurl::*;
#[path = "cli_recall.rs"]
mod cli_recall;
pub use cli_recall::*;
#[path = "cli_triage.rs"]
mod cli_triage;
pub use cli_triage::*;
#[path = "cli_mktsk.rs"]
mod cli_mktsk;
pub use cli_mktsk::*;
#[path = "cli_plan.rs"]
mod cli_plan;
pub use cli_plan::*;
#[path = "cli_checklist.rs"]
mod cli_checklist;
pub use cli_checklist::*;
#[path = "cli_memory.rs"]
mod cli_memory;
pub use cli_memory::*;
#[path = "cli_skill.rs"]
mod cli_skill;
pub use cli_skill::*;
#[path = "cli_verify.rs"]
mod cli_verify;
pub use cli_verify::*;

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

#[path = "cli_commands.rs"]
mod cli_commands;
pub use cli_commands::Commands;

#[derive(Debug, Clone, Args)]
#[command(
    after_help = "Pass additional git push arguments after `--`, for example: csa push origin HEAD -- --tags"
)]
pub struct PushArgs {
    /// Remote to push to (default: origin)
    pub remote: Option<String>,

    /// Refspec to push (default: current branch)
    pub refspec: Option<String>,

    /// Bypass review checks and pass --force to git push
    #[arg(long)]
    pub force: bool,

    /// Pass --force-with-lease to git push while still requiring review coverage
    #[arg(long = "force-with-lease")]
    pub force_with_lease: bool,

    /// Check review coverage without running git push
    #[arg(long)]
    pub check_only: bool,

    /// Extra git push arguments, supplied after `--`
    #[arg(last = true, value_name = "GIT_PUSH_ARG")]
    pub passthrough: Vec<String>,
}

#[derive(Args)]
pub struct MergeArgs {
    /// Pull request number to merge
    #[arg(value_name = "PR_NUMBER", value_parser = clap::value_parser!(u64).range(1..))]
    pub pr_number: u64,

    /// Working directory (defaults to CWD)
    #[arg(long)]
    pub cd: Option<String>,

    /// Base branch to check out and pull after merge (defaults to PR base or main)
    #[arg(long, value_name = "BRANCH")]
    pub base: Option<String>,
}

#[derive(Subcommand)]
pub enum HooksCommands {
    /// Install a `gh` wrapper that intercepts `gh pr merge` before the real binary in PATH
    InstallMergeGuard {
        /// Custom install directory (default: ~/.local/bin/csa-gh-guard)
        #[arg(long, value_name = "DIR")]
        path: Option<PathBuf>,
    },
}

#[derive(clap::Args)]
pub struct ClaudeSubAgentArgs {
    /// Task prompt; reads from stdin if omitted
    pub question: Option<String>,
    /// Autonomous mode flag (REQUIRED for root callers)
    #[arg(long, value_name = "BOOL")]
    pub sa_mode: Option<bool>,
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
    #[arg(long, value_parser = parse_model_spec_arg)]
    pub model_spec: Option<String>,
    /// Working directory
    #[arg(long)]
    pub cd: Option<String>,
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
    /// Set a scalar config value by dotted key path.
    ///
    /// Defaults to the global config. Use --project to write `.csa/config.toml`.
    Set {
        /// Dotted key path (e.g., "preferences.primary_writer_spec")
        key: String,

        /// String value to write
        value: String,

        /// Write project config instead of global config
        #[arg(long, conflicts_with = "global")]
        project: bool,

        /// Write global config (default)
        #[arg(long, conflicts_with = "project")]
        global: bool,

        /// Working directory for --project (defaults to CWD)
        #[arg(long)]
        cd: Option<String>,
    },
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

    /// Install the pre-push review gate (lefthook + review-check.sh) in this repository
    ReviewGate {
        /// Report status of each component without modifying anything
        #[arg(long)]
        check: bool,
    },
}

#[derive(Subcommand)]
pub enum McpHubCommands {
    /// Start MCP Hub service
    Serve {
        /// Launch in background and return immediately
        #[arg(long, conflicts_with = "foreground")]
        background: bool,

        /// Run in foreground mode (default)
        #[arg(long)]
        foreground: bool,

        /// Override hub socket path
        #[arg(long)]
        socket: Option<String>,

        /// HTTP bind host for SSE endpoint
        #[arg(long)]
        http_bind: Option<String>,

        /// HTTP bind port for SSE endpoint (0 = random)
        #[arg(long)]
        http_port: Option<u16>,

        /// Use systemd socket activation (Linux only)
        #[arg(long)]
        systemd_activation: bool,
    },

    /// Check MCP Hub status
    Status {
        /// Override hub socket path
        #[arg(long)]
        socket: Option<String>,
    },

    /// Stop MCP Hub gracefully
    Stop {
        /// Override hub socket path
        #[arg(long)]
        socket: Option<String>,
    },

    /// Regenerate mcp-hub routing-guide skill
    GenSkill {
        /// Override hub socket path
        #[arg(long)]
        socket: Option<String>,
    },
}
