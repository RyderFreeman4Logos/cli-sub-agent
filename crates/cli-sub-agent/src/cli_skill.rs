//! Clap definitions for the `csa skill` subcommand group.

use clap::Subcommand;

#[derive(Subcommand)]
pub enum SkillCommands {
    /// Install skills from a GitHub repository
    Install {
        /// GitHub repo URL or user/repo format (e.g., "user/repo" or "https://github.com/user/repo")
        source: String,

        /// Target tool to install skills for (claude-code, codex, opencode). Defaults to claude-code.
        #[arg(long)]
        target: Option<String>,
    },

    /// List installed skills (active in .claude/skills/ and managed in state dir)
    List,

    /// Run a CSA-managed skill by name
    Run {
        /// Skill name (must exist in the managed skill repo)
        name: String,

        /// Emit the skill prompt to stdout for the calling agent to execute directly,
        /// instead of spawning a CSA session.
        #[arg(long)]
        inject: bool,

        /// Optional prompt to pass to the skill session
        #[arg(trailing_var_arg = true)]
        prompt: Vec<String>,
    },

    /// Add a new skill to the CSA-managed skill repo
    Add {
        /// Skill name (simple identifier, no path separators)
        name: String,
    },

    /// Edit an existing CSA-managed skill in $EDITOR
    Edit {
        /// Skill name
        name: String,
    },

    /// Detect and commit any untracked changes in the managed skill repo
    Scan,

    /// Push the managed skill repo to a private GitHub backup remote
    Backup,
}
