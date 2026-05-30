//! Claude home (`~/.claude` / `$CLAUDE_CONFIG_DIR`) writable-mount wiring.
//!
//! Mirrors [`super::codex_paths::add_codex_home_for_tool`] for the codex home,
//! and exists to fix the nested-EROFS failure in #1661 / #1665 / #1677 / #1683.
//!
//! ## Why every tool's sandbox exposes the claude home
//!
//! Every bwrap sandbox mounts the host root read-only (`--ro-bind / /`) and then
//! re-binds specific paths read-write.  Before this module, the `~/.claude`
//! writable bind was added ONLY in the `"claude-code"` arm of
//! `with_tool_defaults`.  When the PARENT tool is e.g. `codex`, that arm is
//! never entered, so the parent never exposes `~/.claude` writable.  A nested
//! claude-code review/debate child then inherits a read-only `~/.claude` from
//! the parent namespace, and its SessionStart hook
//! `mkdir ~/.claude/session-env/<id>` fails with EROFS.
//!
//! The codebase already solves this exact class for codex:
//! [`super::codex_paths::add_codex_home_for_tool`] exposes `~/.codex` (which
//! holds `auth.json` credentials) writable in EVERY tool's sandbox, "to avoid
//! read-only-fs in nested CSA sessions".  Exposing `~/.claude` writable for all
//! tools therefore EXTENDS that already-accepted symmetry rather than opening a
//! new class of hole: `~/.claude` is the same credential/state risk class as
//! `~/.codex`, which peers already mount read-write.
//!
//! Probe asymmetry mirrors codex exactly: a fail-fast write probe is registered
//! ONLY for the owning tool (`claude-code`); peers get a writable bind but no
//! probe.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::codex_paths::RequiredWritableDir;

/// Environment override claude-code honors to relocate its config/state dir
/// (default `~/.claude`).  Mirrors codex's `CODEX_HOME` handling.
const CLAUDE_CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";
const CLAUDE_DEFAULT_HOME_REL: &str = ".claude";
/// Legacy global config file. With the default layout it lives at
/// `$HOME/.claude.json` — a sibling of `~/.claude`, NOT inside it — so it needs
/// its own writable entry. Under `CLAUDE_CONFIG_DIR` the config file lives
/// inside the (already writable) override dir, so no separate entry is needed.
const CLAUDE_LEGACY_STATE_REL: &str = ".claude.json";
const CLAUDE_SANDBOX_CONFIG_HINT: &str =
    "[tools.claude-code].filesystem_sandbox.writable_paths or [filesystem_sandbox].extra_writable";

/// Expose the claude home (`~/.claude` or `$CLAUDE_CONFIG_DIR`) writable so that
/// a nested claude-code CSA child can create `~/.claude/session-env/<id>`
/// instead of hitting EROFS under a read-only HOME.
///
/// - `tool_name == "claude-code"`: add the claude home writable AND register a
///   fail-fast [`RequiredWritableDir`] probe, plus the legacy `~/.claude.json`
///   state file (default layout only).
/// - any other tool with claude installed: add the claude home writable WITHOUT
///   a probe (the symmetric peer widening — see module docs).
///
/// The peer widening is gated on `has_claude_on_path()` so hosts without claude
/// installed gain nothing.
pub(super) fn add_claude_home_for_tool(
    tool_name: &str,
    home: &Path,
    writable_paths: &mut Vec<PathBuf>,
    required_writable_dirs: &mut Vec<RequiredWritableDir>,
) {
    let (claude_home, claude_home_source) = claude_home_dir(home);
    if tool_name == "claude-code" {
        if claude_home.is_absolute() {
            super::add_dir_or_creatable_parent(writable_paths, &claude_home);
        }
        // Default layout keeps the global config at $HOME/.claude.json (a
        // sibling of ~/.claude, NOT inside it), so it needs its own writable
        // entry. Push it raw: it must stay a file, never a pre-created
        // directory (see isolation_plan_path_tests.rs). Under CLAUDE_CONFIG_DIR
        // the config file lives inside the override dir already covered above.
        if !claude_config_dir_overridden() {
            writable_paths.push(home.join(CLAUDE_LEGACY_STATE_REL));
        }
        required_writable_dirs.push(RequiredWritableDir {
            path: claude_home,
            source: claude_home_source,
            purpose: "Claude session-env recorder and SessionStart hook scratch dir",
            config_hint: CLAUDE_SANDBOX_CONFIG_HINT,
            tool_label: "claude",
        });
    } else if claude_home.is_absolute() && has_claude_on_path() {
        // Claude is installed — route through `add_dir_or_creatable_parent` so
        // the directory is pre-created when it doesn't exist yet (avoids
        // read-only-fs in nested CSA sessions spawning a claude-code child).
        // Symmetric with the `~/.codex` peer widening in isolation_plan_codex.rs.
        super::add_dir_or_creatable_parent(writable_paths, &claude_home);
    }
}

/// Resolve the canonical claude home, honoring `CLAUDE_CONFIG_DIR` when set
/// (mirrors [`super::codex_paths::codex_home_dir`]'s `CODEX_HOME` nuance).
pub(super) fn claude_home_dir(home: &Path) -> (PathBuf, &'static str) {
    match std::env::var_os(CLAUDE_CONFIG_DIR_ENV) {
        Some(value) if !value.is_empty() => (PathBuf::from(value), CLAUDE_CONFIG_DIR_ENV),
        _ => (home.join(CLAUDE_DEFAULT_HOME_REL), "HOME/.claude"),
    }
}

/// Whether `CLAUDE_CONFIG_DIR` relocates the claude home away from the default.
fn claude_config_dir_overridden() -> bool {
    std::env::var_os(CLAUDE_CONFIG_DIR_ENV).is_some_and(|value| !value.is_empty())
}

/// Check whether any claude-code binary (`claude` CLI or the `claude-code-acp`
/// ACP adapter) is on `PATH`.  Mirrors [`super::codex_paths::has_codex_on_path`].
pub(super) fn has_claude_on_path() -> bool {
    for binary in &["claude", "claude-code-acp"] {
        let found = Command::new("which")
            .arg(binary)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if found {
            return true;
        }
    }
    false
}
