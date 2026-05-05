#[derive(clap::Args)]
pub struct MktskArgs {
    /// Task decomposition request
    pub description: String,
    /// TODO plan timestamp to read
    #[arg(long)]
    pub todo: Option<String>,
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
