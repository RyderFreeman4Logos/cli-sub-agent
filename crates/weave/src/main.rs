use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};

mod compiler;
mod package;
mod parser;

/// Weave — skill language compiler and package manager.
#[derive(Parser)]
#[command(name = "weave", version, about)]
struct Cli {
    /// Output format.
    #[arg(long, default_value = "text", global = true)]
    format: Format,

    /// Enable verbose output.
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, ValueEnum)]
enum Format {
    Text,
    Json,
}

#[derive(Subcommand)]
enum Commands {
    /// Compile a weave skill file.
    Compile {
        /// Input file path.
        input: PathBuf,

        /// Output file path.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Install a skill from a git repository.
    Install {
        /// Git URL or user/repo shorthand.
        source: String,
    },

    /// Lock current skill dependencies.
    Lock,

    /// Update a locked dependency.
    Update {
        /// Dependency name to update (all if omitted).
        name: Option<String>,
    },

    /// Audit installed skills for issues.
    Audit,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.verbose {
        tracing_subscriber::fmt()
            .with_env_filter("weave=debug")
            .init();
    }

    match cli.command {
        Commands::Compile { input, output } => {
            let _ = (input, output);
            eprintln!("compile: not yet implemented");
        }
        Commands::Install { source } => {
            let _ = source;
            eprintln!("install: not yet implemented");
        }
        Commands::Lock => {
            eprintln!("lock: not yet implemented");
        }
        Commands::Update { name } => {
            let _ = name;
            eprintln!("update: not yet implemented");
        }
        Commands::Audit => {
            eprintln!("audit: not yet implemented");
        }
    }

    Ok(())
}
