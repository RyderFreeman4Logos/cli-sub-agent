//! Hook system for CSA lifecycle events.
//!
//! Hooks allow customizing behavior at key lifecycle events:
//! - `SessionComplete`: After a session execution completes
//! - `TodoCreate`: After a new TODO plan is created
//! - `TodoSave`: After a TODO plan is saved/updated
//! - `PreRun`: Before a tool execution starts
//! - `PostRun`: After a tool execution finishes
//!
//! ## Configuration Priority
//!
//! Hook configuration is loaded with 4-tier priority:
//! 1. Runtime overrides (CLI params) — highest
//! 2. Project config (`~/.local/state/csa/{project}/hooks.toml`)
//! 3. Global config (`~/.config/cli-sub-agent/hooks.toml`)
//! 4. Built-in defaults — lowest
//!
//! ## Example Config
//!
//! ```toml
//! [session_complete]
//! enabled = true
//! command = "cd {sessions_root} && git add {session_id}/ && git commit -m 'session complete' -q"
//! timeout_secs = 30
//!
//! [todo_create]
//! enabled = true
//! command = "cd {todo_root} && git add {plan_dir}/ && git commit -m 'v1: {plan_id}' -q"
//! ```
//!
//! ## Template Variables
//!
//! Hook commands support template variable substitution with `{variable}` syntax.
//! Variables are shell-escaped to prevent injection.
//!
//! Common variables:
//! - `{session_id}`: Session ULID
//! - `{sessions_root}`: Sessions directory path
//! - `{plan_id}`: TODO plan ULID
//! - `{todo_root}`: TODO root directory path
//! - `{version}`: TODO plan version number
//! - `{message}`: Commit message

pub mod config;
pub mod event;
pub mod guard;
pub mod runner;

// Re-export key types
pub use config::{HookConfig, HooksConfig, global_hooks_path, load_hooks_config};
pub use event::HookEvent;
pub use guard::{
    GuardContext, PromptGuardEntry, PromptGuardResult, format_guard_output, run_prompt_guards,
};
pub use runner::{run_hook, run_hooks_for_event};
