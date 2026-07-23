//! Tests asserting `CSA:CALLER_HINT` emissions are compact and carry
//! the structured session-wait host contract plus cancellation guidance.
//!
//! After #2591, CALLER_HINT markers are compact (≤300 bytes) instead of
//! the previous ~1KB verbose rules. The core contract is:
//!   1. Each daemon entry point emits a wait hint plus a cancellation handle
//!   2. The wait hint contains the re-wait command
//!   3. The cancellation handle contains the exact kill command
//!   4. Session-wait hints state the host execution contract in structured
//!      attributes, including explicit check-in ownership and forbidden APIs
#![cfg(test)]

use std::path::Path;

const NO_POLLING_WARNING: &str = "no polling";
const NO_LOOPS_WARNING: &str = "no loops";

const RUN_CMD_DAEMON_SRC: &str = include_str!("run_cmd_daemon.rs");
const PLAN_CMD_DAEMON_SRC: &str = include_str!("plan_cmd_daemon.rs");
const DAEMON_STARTED_OUTPUT_SRC: &str = include_str!("daemon_started_output.rs");
const SESSION_CMDS_DAEMON_WAIT_SRC: &str = concat!(
    include_str!("session_cmds_daemon_wait.rs"),
    include_str!("session_cmds_daemon_wait_core.rs"),
    include_str!("session_cmds_daemon_wait_completion.rs")
);

/// Extract the body of every `<!-- CSA:CALLER_HINT action=... -->` block in a
/// source string.
///
/// Splits on the literal prefix `<!-- CSA:CALLER_HINT` (the emitted marker is
/// the only legitimate occurrence of this exact prefix) so that incidental
/// mentions of `CSA:CALLER_HINT` in comments or doc-strings cannot produce
/// spurious matches and break the contract assertions in this module.
fn caller_hint_blocks(src: &str) -> Vec<&str> {
    src.split("<!-- CSA:CALLER_HINT")
        .skip(1)
        .map(|tail| {
            let end = tail.find("-->").unwrap_or(tail.len());
            &tail[..end]
        })
        .collect()
}

fn assert_structured_session_wait_hint(marker: &str, action: &str, site: &str) {
    for required_field in [
        format!("action=\"{action}\""),
        "provider=\"xai\"".to_string(),
        "background=true".to_string(),
        "timeout_min_sec=7200".to_string(),
        "notify_on_complete=true".to_string(),
        "checkin_owner=CSA".to_string(),
        "checkin_policy=provider_ttl".to_string(),
        "forbid=\"process.wait,process.poll,manual_status_loops,short_wrapper_timeouts\""
            .to_string(),
    ] {
        assert!(
            marker.contains(&required_field),
            "{site} CALLER_HINT missing required field {required_field}: {marker}"
        );
    }
    for forbidden_pattern in [
        "process.wait",
        "process.poll",
        "manual_status_loops",
        "short_wrapper_timeouts",
    ] {
        assert!(
            marker.contains(forbidden_pattern),
            "{site} CALLER_HINT forbid list omits {forbidden_pattern}: {marker}"
        );
    }
}

#[test]
fn daemon_wait_command_places_cd_after_single_session_id() {
    let session_id = "01KAS6M5XG7V4M4M6YDRS7P8R9";
    let command = crate::daemon_caller_hints::format_session_wait_command(
        session_id,
        Path::new("/tmp/repo"),
        "openai",
    );

    assert_eq!(
        command,
        "csa session wait --session 01KAS6M5XG7V4M4M6YDRS7P8R9 --model-provider openai --cd '/tmp/repo'"
    );
    assert_eq!(
        command.matches(session_id).count(),
        1,
        "session id must appear only in --session"
    );
    assert!(
        !command.contains(&format!("--cd '{session_id}")),
        "session id must not be duplicated into the --cd argument"
    );
}

#[test]
fn daemon_kill_command_keeps_session_and_project_scope() {
    let command = crate::daemon_caller_hints::format_session_kill_command(
        "01KAS6M5XG7V4M4M6YDRS7P8R9",
        Path::new("/receipt-sandbox/project"),
    );

    assert_eq!(
        command,
        "csa session kill --session 01KAS6M5XG7V4M4M6YDRS7P8R9 --cd '/receipt-sandbox/project'"
    );
}

#[test]
fn daemon_wait_command_shell_escapes_project_root_single_quotes() {
    let command = crate::daemon_caller_hints::format_session_wait_command(
        "01KAS6M5XG7V4M4M6YDRS7P8R9",
        Path::new("/tmp/csa'; touch /tmp/csa-review-proof; echo '"),
        "openai",
    );
    assert!(
        command.contains("'\\''; touch /tmp/csa-review-proof; echo '\\'''"),
        "project root single quotes must remain inside the --cd shell argument: {command}"
    );
    assert!(
        !command.contains("--cd '/tmp/csa'; touch"),
        "project root must not terminate the --cd shell argument: {command}"
    );
}

#[test]
fn daemon_caller_hint_attrs_escape_shell_command_values() {
    let command = crate::daemon_caller_hints::format_session_wait_command(
        "01KAS6M5XG7V4M4M6YDRS7P8R9",
        Path::new("/tmp/a\"b&<c>d"),
        "openai",
    );
    let attr = crate::daemon_caller_hints::escape_structured_comment_attr(&command);

    assert_eq!(
        attr,
        "csa session wait --session 01KAS6M5XG7V4M4M6YDRS7P8R9 --model-provider openai --cd '/tmp/a&quot;b&amp;&lt;c&gt;d'"
    );
    assert!(
        !attr.contains('"') && !attr.contains('<') && !attr.contains('>'),
        "escaped attribute must not contain raw XML attribute delimiters: {attr}"
    );
}

#[test]
fn caller_hint_blocks_ignores_unrelated_mentions_in_comments() {
    // A doc/line comment that mentions the marker name MUST NOT be parsed
    // as a hint block; only the exact emitted marker prefix counts.
    let src = r#"
        // see CSA:CALLER_HINT for the wire-format spec
        /// CSA:CALLER_HINT action="wait" — described elsewhere in docs
        fn emit() {
            eprintln!(
                "<!-- CSA:CALLER_HINT action=\"wait\" rule=\"no polling, no loops\" -->"
            );
        }
    "#;
    let blocks = caller_hint_blocks(src);
    assert_eq!(
        blocks.len(),
        1,
        "only the eprintln-emitted hint block must be parsed; comments must be ignored"
    );
    assert!(
        blocks[0].contains("action=\\\"wait\\\""),
        "the parsed block is the real hint, not a comment"
    );
}

#[test]
fn codex_yield_hint_prefers_mcp_wait_and_keeps_shell_fallback_yield() {
    let hint = crate::process_tree::format_codex_yield_hint(
        450_000,
        Some("csa session wait --session <ID> --model-provider openai --cd <PATH>"),
    );

    assert!(hint.contains("mcp_tool=\"csa_session_wait\""), "{hint}");
    assert!(hint.contains("tool_timeout_sec=7200"), "{hint}");
    assert!(hint.contains("timeout_seconds=6900"), "{hint}");
    assert!(
        hint.contains("outer tool_timeout_sec: 7200") && hint.contains("timeout_seconds: 6900"),
        "{hint}"
    );
    assert!(
        hint.contains("Prefer the CSA MCP tool csa_session_wait"),
        "{hint}"
    );
    assert!(hint.contains("Shell fallback only"), "{hint}");
    assert!(hint.contains("yield_time_ms: 450000"), "{hint}");
    assert!(
        !hint.contains("prompt-cache TTL") && !hint.contains("24h"),
        "hint must not claim undocumented Codex cache TTLs: {hint}"
    );
}

/// Maximum allowed length of a single CALLER_HINT block body (bytes).
///
/// CALLER_HINT markers are emitted on every session wait cap (~240s). Over a
/// multi-round review loop (8+ re-waits), verbose markers flood the caller
/// agent's context window with noise, triggering compaction and losing earlier
/// work context (#2591). This budget guards against future bloat.
const CALLER_HINT_MAX_BYTES: usize = 300;

#[test]
fn rendered_session_wait_caller_hints_have_required_host_contract_at_both_sites() {
    for (site, action) in [
        ("daemon_started_output", "wait"),
        ("session_cmds_daemon_wait_completion", "retry_wait"),
    ] {
        let marker = crate::daemon_caller_hints::render_session_wait_caller_hint(action, "xai");
        assert_structured_session_wait_hint(&marker, action, site);
    }
}

#[test]
fn rendered_session_wait_caller_hints_stay_under_size_budget() {
    for action in ["wait", "retry_wait"] {
        let marker = crate::daemon_caller_hints::render_session_wait_caller_hint(action, "xai");
        let rendered_bytes = marker.len();
        assert!(
            rendered_bytes <= CALLER_HINT_MAX_BYTES,
            "rendered {action} CALLER_HINT is {rendered_bytes} bytes, exceeds \
             {CALLER_HINT_MAX_BYTES} byte budget (context flooding guard from #2591): {marker}",
        );
    }
}

#[test]
fn rendered_session_wait_caller_hint_falls_back_when_provider_exceeds_budget() {
    let provider = "x".repeat(200);
    for action in ["wait", "retry_wait"] {
        let marker = crate::daemon_caller_hints::render_session_wait_caller_hint(action, &provider);
        let rendered_bytes = marker.len();
        assert!(
            rendered_bytes <= CALLER_HINT_MAX_BYTES,
            "fallback {action} CALLER_HINT is {rendered_bytes} bytes, exceeds \
             {CALLER_HINT_MAX_BYTES} byte budget: {marker}",
        );
        assert_eq!(
            marker,
            format!(
                "<!-- CSA:CALLER_HINT action=\"{action}\" \
                 forbid=\"process.wait,process.poll,manual_status_loops,short_wrapper_timeouts\" \
                 note=\"budget_exceeded\" -->"
            ),
            "long provider names must use the minimal budget fallback"
        );
    }
}

#[test]
fn run_cmd_daemon_wait_hint_warns_no_stack_wakeup() {
    assert_eq!(
        RUN_CMD_DAEMON_SRC
            .matches("daemon_started_output::prepare")
            .count(),
        1,
        "run_cmd_daemon prepares one shared daemon-start output"
    );
    assert_eq!(
        RUN_CMD_DAEMON_SRC
            .matches("daemon_started_output::publish")
            .count(),
        1,
        "run_cmd_daemon publishes one shared daemon-start output"
    );
    assert!(
        DAEMON_STARTED_OUTPUT_SRC.contains(".caller_hint(\"wait\")"),
        "shared daemon output must render its wait CALLER_HINT through the structured renderer"
    );
}

#[test]
fn daemon_started_output_includes_a_durable_wait_cancellation_handle() {
    let blocks = caller_hint_blocks(DAEMON_STARTED_OUTPUT_SRC);
    let cancellation_blocks = blocks
        .iter()
        .filter(|block| block.contains("action=\\\"cancel_session\\\""))
        .collect::<Vec<_>>();

    assert_eq!(
        cancellation_blocks.len(),
        1,
        "daemon start output must emit one cancellation handle for a background wait"
    );
    let cancellation = cancellation_blocks[0];
    assert!(
        !cancellation.contains("session=\\\"") && !cancellation.contains("kill_cmd=\\\""),
        "cancellation hint must not duplicate the session or kill command already in CSA:SESSION_STARTED: {cancellation}"
    );
    assert!(
        cancellation.contains("does NOT stop the session"),
        "{cancellation}"
    );
    assert!(
        cancellation.contains("CSA:SESSION_STARTED"),
        "cancellation hint must direct callers to the durable kill command: {cancellation}"
    );
}

#[test]
fn plan_cmd_daemon_wait_hint_warns_no_stack_wakeup() {
    assert_eq!(
        PLAN_CMD_DAEMON_SRC
            .matches("daemon_started_output::prepare")
            .count(),
        1,
        "plan_cmd_daemon prepares one shared daemon-start output"
    );
    assert_eq!(
        PLAN_CMD_DAEMON_SRC
            .matches("daemon_started_output::publish")
            .count(),
        1,
        "plan_cmd_daemon publishes one shared daemon-start output"
    );
    assert!(
        DAEMON_STARTED_OUTPUT_SRC.contains(".caller_hint(\"wait\")"),
        "shared daemon output must render its wait CALLER_HINT through the structured renderer"
    );
}

#[test]
fn session_cmds_daemon_wait_retry_wait_hint_warns_no_stack_wakeup() {
    assert!(
        SESSION_CMDS_DAEMON_WAIT_SRC.contains(".caller_hint(\"retry_wait\")"),
        "retry wait output must render its CALLER_HINT through the structured renderer"
    );
}

#[test]
fn session_cmds_daemon_wait_next_session_hint_warns_no_stack_wakeup() {
    let blocks = caller_hint_blocks(SESSION_CMDS_DAEMON_WAIT_SRC);
    let next_blocks = blocks
        .iter()
        .filter(|b| b.contains("action=\\\"next_session\\\""))
        .collect::<Vec<_>>();
    assert_eq!(
        next_blocks.len(),
        1,
        "session_cmds_daemon_wait emits exactly one next_session CALLER_HINT"
    );
    let next = next_blocks[0];
    assert!(
        next.contains(NO_POLLING_WARNING),
        "next_session CALLER_HINT must warn against polling"
    );
    assert!(
        next.contains(NO_LOOPS_WARNING),
        "next_session CALLER_HINT must warn against loops"
    );
}
