// NOTE #1858: #[path]-included by tests; no `crate::`, no binary-only methods (dead_code).
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum DoctorSubcommand {
    /// Verify that the PATH-resolved csa is the artifact installed at the intended target
    Install {
        /// Intended installation target
        #[arg(long, default_value = "/usr/local/bin/csa")]
        target: PathBuf,
        /// Newly built artifact expected to be active; defaults to the intended target
        #[arg(long)]
        artifact: Option<PathBuf>,
    },
    /// Show the complete routing table for all operation types
    Routing {
        /// Filter by operation type (run, review, debate)
        #[arg(long)]
        operation: Option<String>,
        /// Filter by tier name
        #[arg(long)]
        tier: Option<String>,
    },
}
