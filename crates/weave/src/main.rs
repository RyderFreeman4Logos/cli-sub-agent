use std::io::Read;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};

use weave::batch;
use weave::check;
use weave::compiler::{compile, plan_to_toml};
use weave::link::{self, LinkScope};
use weave::package;
use weave::parser::parse_skill;
use weave::visualize::{self, VisualizeResult, VisualizeTarget};

/// Build version string combining Cargo.toml version and git describe.
fn build_version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| {
        let cargo_ver = env!("CARGO_PKG_VERSION");
        let git_desc = env!("WEAVE_GIT_DESCRIBE");
        if git_desc.is_empty() {
            cargo_ver.to_string()
        } else {
            format!("{cargo_ver} ({git_desc})")
        }
    })
}

/// Weave — skill language compiler and package manager.
#[derive(Parser)]
#[command(name = "weave", version = build_version(), about)]
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

/// Where to create skill symlinks after install.
#[derive(Clone, ValueEnum)]
enum LinkScopeArg {
    /// `.claude/skills/` etc. relative to project root.
    Project,
    /// `~/.claude/skills/` etc. relative to home directory.
    User,
    /// Do not create any symlinks.
    None,
}

impl From<LinkScopeArg> for LinkScope {
    fn from(arg: LinkScopeArg) -> Self {
        match arg {
            LinkScopeArg::Project => LinkScope::Project,
            LinkScopeArg::User => LinkScope::User,
            LinkScopeArg::None => LinkScope::None,
        }
    }
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

    /// Install a skill from a git repository or local path.
    Install {
        /// Git URL or user/repo shorthand (mutually exclusive with --path).
        source: Option<String>,

        /// Install from a local directory instead of git.
        #[arg(long, value_name = "DIR", conflicts_with = "source")]
        path: Option<PathBuf>,

        /// Where to create skill symlinks: project (.claude/skills/), user
        /// (~/.claude/skills/), or none (skip linking).
        #[arg(long, default_value = "project")]
        link_scope: LinkScopeArg,

        /// Skip automatic skill symlink creation (alias for --link-scope none).
        #[arg(long, conflicts_with = "link_scope")]
        no_link: bool,

        /// Overwrite existing non-weave symlinks when linking.
        #[arg(long)]
        force_link: bool,
    },

    /// Lock current skill dependencies.
    Lock,

    /// Update a locked dependency.
    Update {
        /// Dependency name to update (all if omitted).
        name: Option<String>,

        /// Force update even for version-pinned dependencies.
        #[arg(long)]
        force: bool,
    },

    /// Upgrade all installed packages to their latest versions.
    ///
    /// Checks each installed package for newer versions and upgrades them.
    /// Reports what was upgraded and what was already at latest.
    Upgrade {
        /// Force upgrade even for version-pinned dependencies.
        #[arg(long)]
        force: bool,
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

    /// Reconcile skill symlinks: create missing, remove stale, fix broken.
    Link {
        #[command(subcommand)]
        action: LinkAction,
    },

    /// Migrate from legacy .weave/lock.toml to weave.lock and global store.
    Migrate,

    /// Garbage-collect unreferenced checkouts from the global package store.
    Gc {
        /// Print what would be removed without actually deleting.
        #[arg(long)]
        dry_run: bool,
    },

    /// Batch-compile all workflow.toml files in a directory tree.
    CompileAll {
        /// Root directory to scan for workflow.toml files (default: patterns/).
        #[arg(long, default_value = "patterns")]
        dir: PathBuf,
    },

    /// Visualize a compiled workflow.toml as ASCII (default), Mermaid, or PNG.
    Visualize {
        /// Input workflow.toml file path.
        plan: PathBuf,

        /// Write PNG output to file.
        #[arg(long, value_name = "FILE", conflicts_with = "mermaid")]
        png: Option<PathBuf>,

        /// Print Mermaid flowchart to stdout.
        #[arg(long, conflicts_with = "png")]
        mermaid: bool,
    },
}

#[derive(Subcommand)]
enum LinkAction {
    /// Reconcile symlinks: create missing, remove stale, fix broken.
    Sync {
        /// Where to manage symlinks: project or user.
        #[arg(long, default_value = "project")]
        scope: LinkScopeArg,

        /// Overwrite existing non-weave symlinks.
        #[arg(long)]
        force: bool,

        /// Show what would be done without making changes.
        #[arg(long)]
        dry_run: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.verbose {
        tracing_subscriber::fmt()
            .with_env_filter("weave=debug")
            .init();
    }

    // Check weave.lock version alignment (non-fatal).
    if let Ok(cwd) = std::env::current_dir() {
        let registry = csa_config::MigrationRegistry::new();
        match csa_config::check_version(
            &cwd,
            env!("CARGO_PKG_VERSION"),
            env!("CARGO_PKG_VERSION"),
            &registry,
        ) {
            Ok(csa_config::VersionCheckResult::MigrationNeeded { pending_count }) => {
                eprintln!(
                    "WARNING: weave.lock is outdated ({pending_count} pending migration(s)). \
                     Run `csa migrate` to update."
                );
            }
            Ok(csa_config::VersionCheckResult::AutoUpdated) => {
                tracing::debug!("weave.lock auto-updated to match binary version");
            }
            Ok(_) => {}
            Err(e) => {
                tracing::debug!("weave.lock version check failed: {e:#}");
            }
        }
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
        Commands::CompileAll { dir } => {
            let summary = batch::compile_all(&dir)?;
            let total = summary.ok + summary.failed;
            if summary.failed > 0 {
                eprintln!(
                    "{total} pattern(s) compiled: {} OK, {} FAILED",
                    summary.ok, summary.failed
                );
                std::process::exit(1);
            } else {
                eprintln!("{total} pattern(s) compiled: {} OK, 0 FAILED", summary.ok);
            }
        }
        Commands::Install {
            source,
            path,
            link_scope,
            no_link,
            force_link,
        } => {
            let project_root = std::env::current_dir().context("cannot determine CWD")?;

            let scope: LinkScope = if no_link {
                LinkScope::None
            } else {
                link_scope.into()
            };

            // Pre-check for link conflicts before installing.
            if scope != LinkScope::None {
                let skills = link::discover_skills(&project_root)?;
                let conflicts = link::precheck_conflicts(&skills);
                if !conflicts.is_empty() {
                    for err in &conflicts {
                        eprintln!("error: {err}");
                    }
                    bail!(
                        "{} skill name conflict(s) detected. \
                         Use --no-link to install without linking, \
                         then create renamed symlinks manually.",
                        conflicts.len()
                    );
                }
            }

            if let Some(local_path) = path {
                let store_root = package::global_store_root()?;
                let pkg = package::install_from_local(&local_path, &project_root, &store_root)?;
                eprintln!("installed {} (local) -> {}/", pkg.name, pkg.name);
            } else if let Some(git_source) = source {
                let cache_root = package::default_cache_root()?;
                let store_root = package::global_store_root()?;
                let pkg = package::install(&git_source, &project_root, &cache_root, &store_root)?;
                let commit_short = &pkg.commit[..pkg.commit.len().min(8)];
                eprintln!(
                    "installed {} ({}) -> {}/{}/",
                    pkg.name, commit_short, pkg.name, commit_short
                );
            } else {
                bail!("either <SOURCE> or --path <DIR> is required");
            }

            // Auto-link companion skills.
            if scope != LinkScope::None {
                let report = link::link_skills(&project_root, scope, force_link)?;
                let created = report.unique_created_count();
                let skipped = report.unique_skipped_count();

                if created > 0 || skipped > 0 {
                    eprintln!("linked {created} skill(s) ({skipped} already up-to-date)");
                }

                for name in report.unique_created_names() {
                    eprintln!("  + {name}");
                }

                if report.has_errors() {
                    for err in &report.errors {
                        eprintln!("error: {err}");
                    }
                    bail!(
                        "{} link error(s) after install — companion skills were NOT linked",
                        report.errors.len()
                    );
                }
            }
        }
        Commands::Lock => {
            let project_root = std::env::current_dir().context("cannot determine CWD")?;
            let store_root = package::global_store_root()?;
            let lockfile = package::lock(&project_root, &store_root)?;
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
        Commands::Update { name, force } => {
            let project_root = std::env::current_dir().context("cannot determine CWD")?;
            let cache_root = package::default_cache_root()?;
            let store_root = package::global_store_root()?;
            let updated = package::update(
                name.as_deref(),
                &project_root,
                &cache_root,
                &store_root,
                force,
            )?;
            for pkg in &updated {
                let commit_short = if pkg.commit.len() > 12 {
                    &pkg.commit[..12]
                } else {
                    &pkg.commit
                };
                eprintln!("updated {} -> {}", pkg.name, commit_short);
            }
        }
        Commands::Upgrade { force } => {
            let project_root = std::env::current_dir().context("cannot determine CWD")?;
            let cache_root = package::default_cache_root()?;
            let store_root = package::global_store_root()?;
            let results = package::upgrade(&project_root, &cache_root, &store_root, force)?;

            let mut upgraded = 0u32;
            let mut already_latest = 0u32;
            let mut skipped = 0u32;

            for entry in &results {
                match &entry.status {
                    package::UpgradeStatus::Upgraded {
                        old_commit,
                        old_version,
                    } => {
                        upgraded += 1;
                        let old_short = &old_commit[..old_commit.len().min(12)];
                        let new_short = &entry.package.commit[..entry.package.commit.len().min(12)];
                        let old_ver = old_version.as_deref().unwrap_or("-");
                        let new_ver = entry.package.version.as_deref().unwrap_or("-");
                        eprintln!(
                            "  upgraded {} ({old_ver} {old_short}) -> ({new_ver} {new_short})",
                            entry.name
                        );
                    }
                    package::UpgradeStatus::AlreadyLatest => {
                        already_latest += 1;
                        let ver = entry.package.version.as_deref().unwrap_or("-");
                        eprintln!("  up-to-date {} ({ver})", entry.name);
                    }
                    package::UpgradeStatus::Skipped { reason } => {
                        skipped += 1;
                        eprintln!("  skipped {} ({reason})", entry.name);
                    }
                }
            }

            eprintln!();
            eprintln!(
                "{} package(s): {upgraded} upgraded, {already_latest} up-to-date, {skipped} skipped",
                results.len()
            );

            // Auto-link companion skills after upgrade.
            if upgraded > 0 {
                let removed = link::remove_stale_links(&project_root, LinkScope::Project)?;
                if !removed.is_empty() {
                    eprintln!("removed {} stale symlink(s)", removed.len());
                }

                let report = link::link_skills(&project_root, LinkScope::Project, true)?;
                let created = report.unique_created_count();
                let link_skipped = report.unique_skipped_count();
                if created > 0 || link_skipped > 0 {
                    eprintln!(
                        "linked {created} skill(s) ({link_skipped} already up-to-date)"
                    );
                }
                for name in report.unique_created_names() {
                    eprintln!("  + {name}");
                }
                if report.has_errors() {
                    for err in &report.errors {
                        eprintln!("warning: {err}");
                    }
                }
            }
        }
        Commands::Audit => {
            let project_root = std::env::current_dir().context("cannot determine CWD")?;
            let store_root = package::global_store_root()?;
            let results = package::audit(&project_root, &store_root)?;
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
        Commands::Migrate => {
            let project_root = std::env::current_dir().context("cannot determine CWD")?;
            let cache_root = package::default_cache_root()?;
            let store_root = package::global_store_root()?;
            match package::migrate(&project_root, &cache_root, &store_root)? {
                package::MigrateResult::AlreadyMigrated => {
                    eprintln!("already migrated — weave.lock exists");
                }
                package::MigrateResult::NothingToMigrate => {
                    eprintln!("nothing to migrate — no .weave/lock.toml found");
                }
                package::MigrateResult::OrphanedDirs(dirs) => {
                    eprintln!(
                        "no lockfile to migrate, but {} legacy director{} found:",
                        dirs.len(),
                        if dirs.len() == 1 { "y" } else { "ies" }
                    );
                    for dir in &dirs {
                        eprintln!("  [!] {}", dir.description);
                        eprintln!("      path: {}", dir.path.display());
                        eprintln!("      fix:  {}", dir.cleanup_hint);
                    }
                    eprintln!();
                    eprintln!(
                        "These directories are not referenced by any lockfile and can \
                         likely be removed safely."
                    );
                    eprintln!("To reinstall packages from scratch, run: weave install <source>");
                }
                package::MigrateResult::Migrated {
                    count,
                    local_skipped,
                    ..
                } => {
                    eprintln!("Migrated {count} package(s) to global store");
                    if local_skipped > 0 {
                        eprintln!(
                            "WARNING: {local_skipped} local-source package(s) were not migrated. \
                             DO NOT remove .weave/deps/ until they are reinstalled."
                        );
                    } else {
                        eprintln!(
                            "You can now safely remove .weave/deps/ with: rm -rf .weave/deps/"
                        );
                    }
                }
            }
        }
        Commands::Gc { dry_run } => {
            let project_root = std::env::current_dir().context("cannot determine CWD")?;
            let store_root = package::global_store_root()?;
            let result = package::gc(&project_root, &store_root, dry_run)?;
            if result.removed.is_empty() {
                eprintln!("nothing to collect — all checkouts are referenced");
            } else if dry_run {
                for entry in &result.removed {
                    eprintln!("  would remove {entry}");
                }
                eprintln!(
                    "would remove {} unreferenced checkout(s), freeing ~{} bytes",
                    result.removed.len(),
                    result.freed_bytes
                );
            } else {
                for entry in &result.removed {
                    eprintln!("  removed {entry}");
                }
                eprintln!(
                    "removed {} unreferenced checkout(s), freed ~{} bytes",
                    result.removed.len(),
                    result.freed_bytes
                );
            }
        }
        Commands::Check { dirs, fix } => {
            let project_root = std::env::current_dir().context("cannot determine CWD")?;
            let scan_dirs = if dirs.is_empty() {
                check::DEFAULT_CHECK_DIRS
                    .iter()
                    .map(PathBuf::from)
                    .collect()
            } else {
                dirs
            };
            let results = check::check_symlinks(&project_root, &scan_dirs, fix)?;
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
        Commands::Link { action } => {
            let project_root = std::env::current_dir().context("cannot determine CWD")?;

            match action {
                LinkAction::Sync {
                    scope,
                    force,
                    dry_run,
                } => {
                    let scope: LinkScope = scope.into();

                    if scope == LinkScope::None {
                        bail!("--scope none is not valid for 'link sync'");
                    }

                    if dry_run {
                        // Dry-run: show what would be done without modifying anything.
                        let skills = link::discover_skills(&project_root)?;
                        eprintln!("would link {} skill(s):", skills.len());
                        for skill in &skills {
                            eprintln!("  {} (from {})", skill.name, skill.package_name);
                        }

                        let stale = link::detect_stale_links(&project_root, scope)?;
                        if stale.is_empty() {
                            eprintln!("no stale links detected");
                        } else {
                            eprintln!("would remove {} stale link(s):", stale.len());
                            for p in &stale {
                                eprintln!("  - {}", p.display());
                            }
                        }
                        eprintln!("(dry-run: no changes made)");
                        return Ok(());
                    }

                    // Remove stale links first.
                    let removed = link::remove_stale_links(&project_root, scope)?;
                    if !removed.is_empty() {
                        eprintln!("removed {} stale symlink(s)", removed.len());
                        for p in &removed {
                            eprintln!("  - {}", p.display());
                        }
                    }

                    // Create/update links.
                    let report = link::link_skills(&project_root, scope, force)?;

                    if report.has_errors() {
                        for err in &report.errors {
                            eprintln!("error: {err}");
                        }
                        bail!("{} error(s) during link sync", report.errors.len());
                    }

                    let created = report.unique_created_count();
                    let skipped = report.unique_skipped_count();
                    eprintln!(
                        "link sync: {created} created, {skipped} up-to-date, {} stale removed",
                        removed.len()
                    );
                }
            }
        }
    }

    Ok(())
}
