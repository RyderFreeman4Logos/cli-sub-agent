//! CLI arguments for the `csa dev2merge` alias.

use clap::Args;

#[derive(Debug, Clone, Args)]
pub struct Dev2mergeArgs {
    /// GitHub issue number to fetch as FEATURE_INPUT
    #[arg(long, value_name = "NUMBER", value_parser = clap::value_parser!(u64).range(1..))]
    pub issue: Option<u64>,

    /// Variable override passed through to the dev2merge workflow
    #[arg(long = "var", value_name = "KEY=VALUE")]
    pub vars: Vec<String>,

    /// Autonomous mode flag (REQUIRED for root callers)
    #[arg(long, value_name = "BOOL")]
    pub sa_mode: Option<bool>,

    /// Set MKTD_TIMEOUT_SECONDS for the dev2merge planning step
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    pub timeout: Option<u64>,
}
