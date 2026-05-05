//! CLI subcommands for the `csa plan` command group.

use clap::Subcommand;
use csa_core::types::ToolName;

#[path = "cli_dev2merge.rs"]
mod cli_dev2merge;
pub use cli_dev2merge::*;

#[derive(Subcommand)]
pub enum PlanCommands {
    /// Execute a weave workflow file
    Run {
        /// Path to workflow TOML file (omit when using --pattern)
        file: Option<String>,

        /// Resolve a pattern by name and use its workflow.toml
        #[arg(long, conflicts_with = "file")]
        pattern: Option<String>,

        /// Autonomous mode flag (REQUIRED for root callers)
        #[arg(long, value_name = "BOOL")]
        sa_mode: Option<bool>,
        /// Variable override (KEY=VALUE, repeatable)
        #[arg(long = "var", value_name = "KEY=VALUE")]
        vars: Vec<String>,

        /// Override tool for all CSA steps (ignores tier routing)
        #[arg(long)]
        tool: Option<ToolName>,

        /// Show execution plan without running
        #[arg(long)]
        dry_run: bool,

        /// Execute only one step then return (caller polls with --resume)
        #[arg(long)]
        chunked: bool,

        /// Resume from a journal state file (path to journal JSON file)
        #[arg(long, conflicts_with = "file", conflicts_with = "pattern")]
        resume: Option<String>,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,

        /// Block instead of daemonizing (auto for --dry-run/--chunked/--resume).
        #[arg(long)]
        foreground: bool,
        #[arg(long, hide = true)]
        daemon_child: bool,
        #[arg(long, hide = true)]
        session_id: Option<String>,
    },
}
