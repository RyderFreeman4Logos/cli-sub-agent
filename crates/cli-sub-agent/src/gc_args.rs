// NOTE #1858: #[path]-included by tests; no `crate::`, no binary-only methods (dead_code).
#[derive(Debug, clap::Args, Clone)]
pub struct GcArgs {
    /// Show what would be removed without actually removing
    #[arg(long)]
    pub dry_run: bool,

    /// Age threshold in days. Deletes whole sessions by default; with
    /// `--reap-runtime`, only removes the session's `runtime/` subtree.
    #[arg(long)]
    pub max_age_days: Option<u64>,

    /// Reap completed sessions' `runtime/` payload instead of deleting the
    /// entire session directory.
    #[arg(long, requires = "max_age_days")]
    pub reap_runtime: bool,

    /// Scan all projects under ~/.local/state/cli-sub-agent/ (not just current project)
    #[arg(long)]
    pub global: bool,

    /// Working directory (defaults to CWD)
    #[arg(long)]
    pub cd: Option<String>,
}
