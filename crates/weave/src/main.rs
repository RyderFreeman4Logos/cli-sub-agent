use std::io::Read;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use weave::compiler::{compile, plan_to_toml};
use weave::package;
use weave::parser::parse_skill;
use weave::visualize::{self, VisualizeResult, VisualizeTarget};

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

    /// Check for broken symlinks in skill directories.
    Check {
        /// Directories to scan (default: .claude/skills, .codex/skills, .agents/skills, .gemini/skills).
        #[arg(long = "dir", value_name = "DIR")]
        dirs: Vec<PathBuf>,

        /// Remove broken symlinks.
        #[arg(long)]
        fix: bool,
    },

    /// Visualize a compiled plan.toml as ASCII (default), Mermaid, or PNG.
    Visualize {
        /// Input plan.toml file path.
        plan: PathBuf,

        /// Write PNG output to file.
        #[arg(long, value_name = "FILE", conflicts_with = "mermaid")]
        png: Option<PathBuf>,

        /// Print Mermaid flowchart to stdout.
        #[arg(long, conflicts_with = "png")]
        mermaid: bool,
    },
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
            let project_root = std::env::current_dir().context("cannot determine CWD")?;
            let cache_root = package::default_cache_root()?;
            let pkg = package::install(&source, &project_root, &cache_root)?;
            eprintln!(
                "installed {} ({}) -> .weave/deps/{}/",
                pkg.name,
                &pkg.commit[..pkg.commit.len().min(12)],
                pkg.name
            );
        }
        Commands::Lock => {
            let project_root = std::env::current_dir().context("cannot determine CWD")?;
            let lockfile = package::lock(&project_root)?;
            eprintln!("locked {} package(s)", lockfile.package.len());
            for pkg in &lockfile.package {
                let ver = pkg.version.as_deref().unwrap_or("-");
                let commit_short = if pkg.commit.len() > 12 {
                    &pkg.commit[..12]
                } else {
                    &pkg.commit
                };
                eprintln!("  {} {} ({})", pkg.name, ver, commit_short);
            }
        }
        Commands::Update { name } => {
            let project_root = std::env::current_dir().context("cannot determine CWD")?;
            let cache_root = package::default_cache_root()?;
            let updated = package::update(name.as_deref(), &project_root, &cache_root)?;
            for pkg in &updated {
                let commit_short = if pkg.commit.len() > 12 {
                    &pkg.commit[..12]
                } else {
                    &pkg.commit
                };
                eprintln!("updated {} -> {}", pkg.name, commit_short);
            }
        }
        Commands::Audit => {
            let project_root = std::env::current_dir().context("cannot determine CWD")?;
            let results = package::audit(&project_root)?;
            if results.is_empty() {
                eprintln!("audit passed: no issues found");
            } else {
                for result in &results {
                    for issue in &result.issues {
                        eprintln!("  [!] {}: {}", result.name, issue);
                    }
                }
                std::process::exit(1);
            }
        }
        Commands::Check { dirs, fix } => {
            let project_root = std::env::current_dir().context("cannot determine CWD")?;
            let scan_dirs = if dirs.is_empty() {
                package::DEFAULT_CHECK_DIRS
                    .iter()
                    .map(PathBuf::from)
                    .collect()
            } else {
                dirs
            };
            let results = package::check_symlinks(&project_root, &scan_dirs, fix)?;
            let total_broken: usize = results.iter().map(|r| r.issues.len()).sum();
            let total_fixed: usize = results.iter().map(|r| r.fixed).sum();
            let total_failures: usize = results.iter().map(|r| r.fix_failures).sum();

            if total_broken == 0 {
                eprintln!("check passed: no broken symlinks found");
            } else {
                for result in &results {
                    for issue in &result.issues {
                        eprintln!("  [!] {issue}");
                    }
                }
                if fix {
                    eprintln!("fixed {total_fixed} broken symlink(s)");
                    if total_failures > 0 {
                        eprintln!(
                            "warning: failed to remove {total_failures} symlink(s) (permission denied?)"
                        );
                        std::process::exit(1);
                    }
                } else {
                    eprintln!("found {total_broken} broken symlink(s) — run with --fix to remove");
                    std::process::exit(1);
                }
            }
        }
        Commands::Visualize { plan, png, mermaid } => {
            let target = if mermaid {
                VisualizeTarget::Mermaid
            } else if let Some(output) = png {
                VisualizeTarget::Png(output)
            } else {
                VisualizeTarget::Ascii
            };

            let result = if plan.as_os_str() == "-" {
                let mut content = String::new();
                std::io::stdin()
                    .read_to_string(&mut content)
                    .context("failed to read stdin")?;
                visualize::visualize_plan_toml(&content, "stdin", target)?
            } else {
                visualize::visualize_plan_file(&plan, target)?
            };

            match result {
                VisualizeResult::Stdout(rendered) => {
                    print!("{rendered}");
                }
                VisualizeResult::FileWritten(path) => {
                    eprintln!("wrote {}", path.display());
                }
            }
        }
    }

    Ok(())
}
