//! Completion status and exit code determination for `csa session wait`.
//!
//! Extracted from `session_cmds_daemon_wait.rs` to reduce module complexity.

use std::borrow::Cow;
use std::path::Path;

use chrono::{DateTime, SecondsFormat, Utc};

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

pub(crate) struct WaitCapContext<'a> {
    pub(crate) project_root: &'a Path,
    pub(crate) preferred_provider: Option<&'a csa_config::ModelProvider>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WaitProgressDigest {
    elapsed_secs: u64,
    tools: String,
    last_event: DateTime<Utc>,
}

impl WaitProgressDigest {
    pub(crate) fn from_session_dir(session_dir: &Path) -> Option<Self> {
        let state = read_session_state(session_dir)?;
        let tool = read_session_tool(session_dir).or_else(|| tool_from_state(&state));
        Some(Self::from_state_and_tool(
            Utc::now(),
            &state,
            tool.as_deref(),
        ))
    }

    fn from_state_and_tool(
        now: DateTime<Utc>,
        state: &csa_session::MetaSessionState,
        tool: Option<&str>,
    ) -> Self {
        let elapsed_secs = now
            .signed_duration_since(state.created_at)
            .num_seconds()
            .max(0) as u64;
        Self {
            elapsed_secs,
            tools: compact_progress_value(tool.unwrap_or("unknown")),
            last_event: state.last_accessed,
        }
    }

    pub(crate) fn render(&self) -> String {
        format!(
            "Progress: elapsed={} tools={} last_event={}",
            format_elapsed_compact(self.elapsed_secs),
            self.tools,
            self.last_event.to_rfc3339_opts(SecondsFormat::Secs, true),
        )
    }
}

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
    context: WaitCapContext<'_>,
    wait_timeout_secs: u64,
    elapsed: u64,
    session_dir: &Path,
    session_alive: bool,
) -> i32 {
    let cd_arg = cd
        .map(|path| crate::daemon_caller_hints::format_cd_arg(Path::new(path)))
        .unwrap_or_default();
    if session_alive {
        let wait_command = crate::daemon_caller_hints::resolve_session_wait_command(
            session_id,
            context.project_root,
            context.preferred_provider,
        );
        eprintln!(
            "Session {session_id} still running after {wait_timeout_secs}s wait cap; returning so caller can warm its KV cache before re-waiting."
        );
        if let Some(progress) = WaitProgressDigest::from_session_dir(session_dir) {
            eprintln!("{}", progress.render());
        }
        if let Some(wait_cmd) = wait_command.command() {
            let wait_cmd_attr =
                crate::daemon_caller_hints::escape_structured_comment_attr(wait_cmd);
            eprintln!(
                "<!-- CSA:SESSION_WAIT_KV_WARM session={session_id} status=alive elapsed={elapsed}s action=re-wait cmd=\"{wait_cmd_attr}\" -->"
            );
            eprintln!(
                "<!-- CSA:CALLER_HINT action=\"retry_wait\" rule=\"Session alive; re-wait in a NEW Bash call: {wait_cmd_attr}. Backgrounded? Task-notification is your wake signal — no polling, no loops.\" -->"
            );
            let codex_hint = crate::process_tree::codex_yield_hint(Some(wait_cmd));
            if !codex_hint.is_empty() {
                eprint!("{codex_hint}");
            }
        } else {
            eprintln!(
                "<!-- CSA:SESSION_WAIT_KV_WARM session={session_id} status=alive elapsed={elapsed}s action=select_wait_provider -->"
            );
            eprintln!("{}", wait_command.provider_selection_hint());
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

fn read_session_state(session_dir: &Path) -> Option<csa_session::MetaSessionState> {
    let raw = std::fs::read_to_string(session_dir.join("state.toml")).ok()?;
    toml::from_str(&raw).ok()
}

fn read_session_tool(session_dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(session_dir.join(csa_session::metadata::METADATA_FILE_NAME))
        .ok()?;
    let metadata: csa_session::metadata::SessionMetadata = toml::from_str(&raw).ok()?;
    let tool = metadata.tool.trim();
    (!tool.is_empty()).then(|| tool.to_string())
}

fn tool_from_state(state: &csa_session::MetaSessionState) -> Option<String> {
    let mut tools: Vec<&str> = state.tools.keys().map(String::as_str).collect();
    tools.sort_unstable();
    (!tools.is_empty()).then(|| tools.join(","))
}

fn compact_progress_value(value: &str) -> String {
    let compacted: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ',' | '/') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let compacted = compacted.trim_matches('-');
    if compacted.is_empty() {
        "unknown".to_string()
    } else {
        compacted.to_string()
    }
}

fn format_elapsed_compact(seconds: u64) -> String {
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    let remaining_minutes = minutes % 60;
    if remaining_minutes == 0 {
        format!("{hours}h")
    } else {
        format!("{hours}h{remaining_minutes}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn progress_digest_renders_elapsed_tool_and_last_event() {
        let state = csa_session::MetaSessionState {
            created_at: Utc.with_ymd_and_hms(2026, 6, 30, 5, 25, 0).unwrap(),
            last_accessed: Utc.with_ymd_and_hms(2026, 6, 30, 5, 34, 0).unwrap(),
            ..Default::default()
        };

        let now = Utc.with_ymd_and_hms(2026, 6, 30, 5, 34, 30).unwrap();
        let digest = WaitProgressDigest::from_state_and_tool(now, &state, Some("codex"));

        assert_eq!(
            digest.render(),
            "Progress: elapsed=9m tools=codex last_event=2026-06-30T05:34:00Z"
        );
    }

    #[test]
    fn format_elapsed_compact_bounds_to_one_field() {
        assert_eq!(format_elapsed_compact(59), "59s");
        assert_eq!(format_elapsed_compact(60), "1m");
        assert_eq!(format_elapsed_compact(3_660), "1h1m");
    }
}
