#[derive(clap::Args)]
pub struct ArchArgs {
    /// Architecture analysis description
    pub description: String,
    /// Tool to use
    #[arg(long)]
    pub tool: Option<String>,
    /// Session timeout in seconds
    #[arg(long, default_value = "7200")]
    pub timeout: u64,
    /// Allow working on base branch
    #[arg(long, alias = "allow-base-branch-commit")]
    pub allow_base_branch_working: bool,
}
