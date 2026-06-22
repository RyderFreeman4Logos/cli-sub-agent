//! Completion status and exit code determination for `csa session wait`.
//!
//! Extracted from `session_cmds_daemon_wait.rs` to reduce module complexity.

use std::borrow::Cow;
use std::path::Path;

/// Exit code reserved for `csa session wait` memory warning early-exit.
pub(crate) const SESSION_WAIT_MEMORY_WARN_EXIT_CODE: i32 = 33;
pub(crate) const SESSION_WAIT_SUCCESS_EXIT_CODE: i32 = 0;
pub(crate) const SESSION_WAIT_FAILURE_EXIT_CODE: i32 = 1;
/// Healthy poll-cap exit when the session is still alive: callers should
/// process tokens (warming their KV cache) and re-wait. See #1439.
pub(crate) const SESSION_WAIT_KV_WARM_EXIT_CODE: i32 = 0;
/// Reserved for the rare case where the wait cap is reached but the session
/// daemon is no longer alive and no result.toml was produced.
pub(crate) const SESSION_WAIT_TIMEOUT_EXIT_CODE: i32 = 124;

/// Determine completion status string and exit code from session result.
pub(crate) fn resolve_wait_completion_status_and_exit<'a>(
    fallback_status: &'a str,
    fallback_exit_code: i32,
    synthetic: bool,
    real_result: Option<&'a csa_session::SessionResult>,
) -> (Cow<'a, str>, i32) {
    if synthetic {
        return (Cow::Borrowed("failure"), SESSION_WAIT_FAILURE_EXIT_CODE);
    }
    real_result.map_or_else(
        || {
            (
                Cow::Borrowed(fallback_status),
                terminal_result_wait_exit_code(fallback_status, fallback_exit_code),
            )
        },
        |result| {
            (
                Cow::Borrowed(result.status.as_str()),
                terminal_result_wait_exit_code(result.status.as_str(), result.exit_code),
            )
        },
    )
}

/// Convert session result status/exit_code to `csa session wait` exit code.
pub(crate) fn terminal_result_wait_exit_code(status: &str, exit_code: i32) -> i32 {
    if matches!(status, "success" | "retired") && exit_code == 0 {
        SESSION_WAIT_SUCCESS_EXIT_CODE
    } else {
        SESSION_WAIT_FAILURE_EXIT_CODE
    }
}

pub(crate) fn emit_wait_cap_outcome(
    session_id: &str,
    cd: Option<&str>,
    wait_timeout_secs: u64,
    elapsed: u64,
    session_alive: bool,
) -> i32 {
    let cd_arg = cd
        .map(|path| crate::daemon_caller_hints::format_cd_arg(Path::new(path)))
        .unwrap_or_default();
    if session_alive {
        let wait_cmd = format!("csa session wait --session {session_id}{cd_arg}");
        let wait_cmd_attr = crate::daemon_caller_hints::escape_structured_comment_attr(&wait_cmd);
        eprintln!(
            "Session {session_id} still running after {wait_timeout_secs}s wait cap; returning so caller can warm its KV cache before re-waiting."
        );
        eprintln!(
            "<!-- CSA:SESSION_WAIT_KV_WARM session={session_id} status=alive elapsed={elapsed}s action=re-wait cmd=\"{wait_cmd_attr}\" -->"
        );
        eprintln!(
            "<!-- CSA:CALLER_HINT action=\"retry_wait\" \
             rule=\"Session is alive; this exit is healthy. Process this output NOW and generate tokens to warm your KV cache, \
             then call {wait_cmd} again in a NEW Bash call. \
             NEVER batch multiple session waits in one Bash call. \
             If you background the wait (run_in_background: true), the completion task-notification IS your wake signal — do NOT stack ScheduleWakeup, /loop, or sleep loops on top; that's pure redundancy and wastes tokens. \
             FORBIDDEN: ls/cat/wc/grep on session-dir, state.toml reads, ps checks on daemon PID — \
             any manual polling wastes caller tokens with zero benefit. \
             FORBIDDEN: piping csa commands through 2>/dev/null. CSA errors on stderr are diagnostic — \
             suppressing them hides invalid-argument errors and causes silent retry loops that waste thousands of tokens.\" -->",
            wait_cmd = wait_cmd_attr,
        );
        let codex_hint = crate::process_tree::codex_yield_hint();
        if !codex_hint.is_empty() {
            eprint!("{codex_hint}");
        }
        SESSION_WAIT_KV_WARM_EXIT_CODE
    } else {
        eprintln!(
            "Timeout: session {session_id} did not complete within {wait_timeout_secs}s and no live daemon process remains."
        );
        let result_cmd = format!("csa session result --session {session_id}{cd_arg}");
        let result_cmd_attr =
            crate::daemon_caller_hints::escape_structured_comment_attr(&result_cmd);
        eprintln!(
            "<!-- CSA:SESSION_WAIT_TIMEOUT session={session_id} elapsed={elapsed}s status=dead cmd=\"{result_cmd_attr}\" -->"
        );
        SESSION_WAIT_TIMEOUT_EXIT_CODE
    }
}
