// NOTE #1858: #[path]-included by tests; no `crate::`, no binary-only methods (dead_code).
//! CLI subcommands for the `csa plan` command group.

use clap::Subcommand;
use csa_core::types::ToolName;

use super::parse_model_spec_arg;

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

        /// Fetch a GitHub issue body and inject it as the FEATURE_INPUT
        /// workflow variable (conflicts with an explicit --var FEATURE_INPUT=...).
        #[arg(long, value_name = "NUMBER", value_parser = clap::value_parser!(u64).range(1..))]
        issue: Option<u64>,

        /// Override tool for all CSA steps (ignores tier routing)
        #[arg(long)]
        tool: Option<ToolName>,

        /// Override model spec for all CSA steps (tool/provider/model/thinking format)
        #[arg(long, value_parser = parse_model_spec_arg)]
        model_spec: Option<String>,

        /// Show execution plan without running
        #[arg(long)]
        dry_run: bool,

        /// Execute only one step then return (caller polls with --resume)
        #[arg(long)]
        chunked: bool,

        /// Resume from a journal state file (path to journal JSON file)
        #[arg(long, conflicts_with = "file", conflicts_with = "pattern")]
        resume: Option<String>,

        /// Mark the pending manual step complete before resuming
        #[arg(
            long,
            value_name = "STEP_ID",
            requires = "resume",
            conflicts_with = "dry_run"
        )]
        complete_manual_step: Option<usize>,

        /// Working directory
        #[arg(long)]
        cd: Option<String>,

        /// Disable filesystem sandbox isolation (bwrap/landlock)
        #[arg(long)]
        no_fs_sandbox: bool,

        /// Block instead of daemonizing (auto for --dry-run/--chunked/--resume).
        #[arg(long)]
        foreground: bool,
        #[arg(long, hide = true)]
        daemon_child: bool,
        #[arg(long, hide = true)]
        session_id: Option<String>,
    },
}
