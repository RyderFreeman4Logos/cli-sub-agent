use anyhow::Result;
use clap::Parser;

mod cli;

use cli::{Cli, Commands};

fn main() -> Result<()> {
    // Read current depth from env
    let current_depth: u32 = std::env::var("CSA_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    // Hard limit recursion depth (default 5, config loading comes later)
    let max_depth: u32 = 5;
    if current_depth > max_depth {
        eprintln!(
            "Error: Max recursion depth ({}) exceeded. Current: {}. Do it yourself.",
            max_depth, current_depth
        );
        std::process::exit(1);
    }

    // Initialize tracing (output to stderr, initialize only once)
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run { .. } => {
            eprintln!("Run command not yet implemented");
        }
        Commands::Session { .. } => {
            eprintln!("Session command not yet implemented");
        }
        Commands::Init { .. } => {
            eprintln!("Init command not yet implemented");
        }
        Commands::Gc => {
            eprintln!("Gc command not yet implemented");
        }
        Commands::Config { .. } => {
            eprintln!("Config command not yet implemented");
        }
    }

    Ok(())
}
