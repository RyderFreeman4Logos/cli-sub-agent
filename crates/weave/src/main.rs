use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use weave::compiler::{compile, plan_to_toml};
use weave::parser::parse_skill;

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
    /// Compile a weave skill file into an execution plan.
    Compile {
        /// Input Markdown file path.
        input: PathBuf,

        /// Output TOML file path (stdout if omitted).
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
            let content = std::fs::read_to_string(&input)
                .with_context(|| format!("failed to read {}", input.display()))?;
            let doc = parse_skill(&content)
                .with_context(|| format!("failed to parse {}", input.display()))?;
            let plan = compile(&doc).context("compilation failed")?;
            let toml_str = plan_to_toml(&plan)?;

            if let Some(out_path) = output {
                std::fs::write(&out_path, &toml_str)
                    .with_context(|| format!("failed to write {}", out_path.display()))?;
                eprintln!("wrote {}", out_path.display());
            } else {
                print!("{toml_str}");
            }
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
