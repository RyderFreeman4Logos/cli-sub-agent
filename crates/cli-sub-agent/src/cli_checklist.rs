use clap::Subcommand;

#[derive(Subcommand)]
pub enum ChecklistCommands {
    /// Show current branch's review checklist
    Show {
        #[arg(long)]
        cd: Option<String>,
        #[arg(short, long)]
        branch: Option<String>,
    },
    /// Check a criterion (mark as passed with evidence)
    Check {
        /// Criterion ID to check
        id: String,
        /// Evidence supporting the check
        #[arg(long)]
        evidence: String,
        /// Reviewer name/model
        #[arg(long)]
        reviewer: Option<String>,
        #[arg(long)]
        cd: Option<String>,
        #[arg(short, long)]
        branch: Option<String>,
    },
    /// Reset a criterion back to unchecked
    Reset {
        /// Criterion ID to reset
        id: String,
        #[arg(long)]
        cd: Option<String>,
        #[arg(short, long)]
        branch: Option<String>,
    },
    /// Generate a checklist from AGENTS.md rules for the current profile
    Generate {
        /// Project profile (rust, node, python, go, mixed)
        #[arg(long)]
        profile: Option<String>,
        /// Diff scope (e.g., base:main)
        #[arg(long)]
        scope: Option<String>,
        #[arg(long)]
        cd: Option<String>,
        #[arg(short, long)]
        branch: Option<String>,
    },
    /// List all active checklists
    List {
        #[arg(long)]
        cd: Option<String>,
    },
}
