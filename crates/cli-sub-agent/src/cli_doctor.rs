use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum DoctorSubcommand {
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
