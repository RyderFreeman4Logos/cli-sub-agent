//! Completion status and exit code determination for `csa session wait`.
//!
//! Extracted from `session_cmds_daemon_wait.rs` to reduce module complexity.

use std::borrow::Cow;

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
