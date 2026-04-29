//! Hook system for CSA lifecycle events.
//!
//! Hooks allow customizing behavior at key lifecycle events:
//! - `PreSession`: Before the first user message is sent to a resolved transport
//! - `SessionComplete`: After a session execution completes
//! - `TodoCreate`: After a new TODO plan is created
//! - `TodoSave`: After a TODO plan is saved/updated
//! - `PreRun`: Before a tool execution starts
//! - `PostRun`: After a tool execution finishes
//! - `PostEdit`: After PostRun when `.rs` files changed (observational clippy check)
//! - `MergeCompleted`: After merge_guard allows a merge to proceed (audit event)
//!
//! ## Configuration Priority
//!
//! Most hook configuration is loaded with 4-tier priority:
//! 1. Runtime overrides (CLI params) — highest
//! 2. Project config (`~/.local/state/cli-sub-agent/{project}/hooks.toml`)
//! 3. Global config (`~/.config/cli-sub-agent/hooks.toml`)
//! 4. Built-in defaults — lowest
//!
//! `PreSession` is global-only and is loaded from
//! `~/.config/cli-sub-agent/config.toml` under `[hooks.pre_session]`.
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

pub mod audit;
pub mod config;
pub mod directive;
pub mod event;
pub mod event_bus;
pub mod guard;
pub mod mempal_capture;
pub mod merge_guard;
pub mod policy;
pub mod pre_session;
pub mod runner;
pub mod waiver;

// Re-export key types
pub use audit::{MergeAuditEvent, audit_log_path, emit_merge_completed_event};
pub use config::{HookConfig, HooksConfig, global_hooks_path, load_hooks_config};
pub use directive::{
    NextStepDirective, format_next_step_directive, parse_next_step, parse_next_step_directive,
};
pub use event::HookEvent;
#[cfg(feature = "async-hooks")]
pub use event_bus::AsyncEventBus;
pub use event_bus::{EventBus, SyncEventBus};
pub use guard::{
    GuardContext, PromptGuardEntry, PromptGuardResult, builtin_prompt_guards, format_guard_output,
    run_prompt_guards,
};
pub use merge_guard::{
    MarkerStatus, default_install_dir, detect_installed_guard, ensure_guard_dir, gh_wrapper_script,
    inject_merge_guard_env, install_merge_guard, is_merge_guard_enabled, verify_pr_bot_marker,
};
pub use policy::FailPolicy;
pub use pre_session::{
    PreSessionHookConfig, PreSessionHookContext, PreSessionHookInvocation,
    format_pre_session_reminder, global_pre_session_config_path,
    load_global_pre_session_hook_config, load_global_pre_session_hook_invocation,
    load_pre_session_hook_config_from_path, parse_pre_session_hook_config,
    prepend_pre_session_stdout, run_pre_session_hook,
};
pub use runner::{run_hook, run_hook_capturing, run_hooks_for_event};
pub use waiver::{Waiver, WaiverSet};

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{LazyLock, Mutex};

    /// Process-wide lock for tests that mutate shared process environment.
    pub(crate) static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
}
