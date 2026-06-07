// NOTE #1858: #[path]-included by tests; no `crate::`, no binary-only methods (dead_code).
use std::path::PathBuf;

use clap::{Args, ValueEnum};

#[derive(Debug, Clone, Args)]
pub struct VerifyArgs {
    /// Falsifiable claim to verify
    #[arg(long)]
    pub claim: String,

    /// Git ref for the baseline
    #[arg(long, default_value = "main")]
    pub baseline: String,

    /// Git ref for the treatment
    #[arg(long, default_value = "HEAD")]
    pub treatment: String,

    /// Verification method; omitted means auto-detect from the claim text
    #[arg(long, value_enum)]
    pub method: Option<VerifyMethodArg>,

    /// Write the structured JSON result to this path
    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, ValueEnum)]
pub enum VerifyMethodArg {
    Test,
    Benchmark,
    TokenCount,
    Checklist,
}
