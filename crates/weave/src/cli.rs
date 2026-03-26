use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use weave::link::LinkScope;

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
pub struct Cli {
    /// Output format.
    #[arg(long, default_value = "text", global = true)]
    pub format: Format,

    /// Enable verbose output.
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Clone, ValueEnum)]
pub enum Format {
    Text,
    Json,
}

/// Where to create skill symlinks after install.
#[derive(Clone, ValueEnum)]
pub enum LinkScopeArg {
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
pub enum Commands {
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

    /// Migrate `.gemini/skills/` symlinks to `.agents/skills/` (move unique, remove duplicates).
    CleanGeminiSkills,

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
pub enum LinkAction {
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
