//! Tests asserting `CSA:CALLER_HINT` emissions are compact and carry
//! the essential action/re-wait guidance.
//!
//! After #2591, CALLER_HINT markers are compact (≤200 bytes) instead of
//! the previous ~1KB verbose rules. The core contract is:
//!   1. Each daemon entry point emits exactly one CALLER_HINT
//!   2. The hint contains the re-wait command or action
//!   3. The hint warns against polling and loops
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

fn assert_wait_hint_contract(block: &str, site: &str) {
    assert!(
        block.contains("action=\\\"wait\\\""),
        "{site} emits action=\"wait\""
    );
    assert!(
        block.contains("run_in_background"),
        "{site} CALLER_HINT must recommend backgrounding session wait"
    );
    assert!(
        block.contains(NO_POLLING_WARNING),
        "{site} CALLER_HINT must warn against polling"
    );
    assert!(
        block.contains(NO_LOOPS_WARNING),
        "{site} CALLER_HINT must warn against loops"
    );
}

#[test]
fn daemon_wait_command_places_cd_after_single_session_id() {
    let session_id = "01KAS6M5XG7V4M4M6YDRS7P8R9";
    let command =
        crate::daemon_caller_hints::format_session_wait_command(session_id, Path::new("/tmp/repo"));

    assert_eq!(
        command,
        "csa session wait --session 01KAS6M5XG7V4M4M6YDRS7P8R9 --cd '/tmp/repo'"
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
fn daemon_wait_command_shell_escapes_project_root_single_quotes() {
    let command = crate::daemon_caller_hints::format_session_wait_command(
        "01KAS6M5XG7V4M4M6YDRS7P8R9",
        Path::new("/tmp/csa'; touch /tmp/csa-review-proof; echo '"),
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
    );
    let attr = crate::daemon_caller_hints::escape_structured_comment_attr(&command);

    assert_eq!(
        attr,
        "csa session wait --session 01KAS6M5XG7V4M4M6YDRS7P8R9 --cd '/tmp/a&quot;b&amp;&lt;c&gt;d'"
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
    let hint = crate::process_tree::format_codex_yield_hint(450_000);

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
fn caller_hint_blocks_stay_under_size_budget() {
    let all_sources = [
        ("daemon_started_output", DAEMON_STARTED_OUTPUT_SRC),
        ("session_cmds_daemon_wait", SESSION_CMDS_DAEMON_WAIT_SRC),
    ];
    for (site, src) in &all_sources {
        for block in caller_hint_blocks(src) {
            assert!(
                block.len() <= CALLER_HINT_MAX_BYTES,
                "{site} CALLER_HINT block is {} bytes, exceeds {} byte budget \
                 (context flooding guard from #2591). Block: {block}",
                block.len(),
                CALLER_HINT_MAX_BYTES,
            );
        }
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
    let blocks = caller_hint_blocks(DAEMON_STARTED_OUTPUT_SRC);
    assert_eq!(
        blocks.len(),
        1,
        "shared daemon output emits one CALLER_HINT"
    );
    assert_wait_hint_contract(blocks[0], "run_cmd_daemon");
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
    let blocks = caller_hint_blocks(DAEMON_STARTED_OUTPUT_SRC);
    assert_eq!(
        blocks.len(),
        1,
        "shared daemon output emits one CALLER_HINT"
    );
    assert_wait_hint_contract(blocks[0], "plan_cmd_daemon");
}

#[test]
fn session_cmds_daemon_wait_retry_wait_hint_warns_no_stack_wakeup() {
    let blocks = caller_hint_blocks(SESSION_CMDS_DAEMON_WAIT_SRC);
    assert!(
        blocks.len() >= 2,
        "session_cmds_daemon_wait emits both retry_wait and next_session hints; got {} blocks",
        blocks.len()
    );
    let retry_blocks = blocks
        .iter()
        .filter(|b| b.contains("action=\\\"retry_wait\\\""))
        .collect::<Vec<_>>();
    assert_eq!(
        retry_blocks.len(),
        1,
        "session_cmds_daemon_wait emits exactly one retry_wait CALLER_HINT"
    );
    let retry = retry_blocks[0];
    assert!(
        retry.contains(NO_POLLING_WARNING),
        "retry_wait CALLER_HINT must warn against polling"
    );
    assert!(
        retry.contains(NO_LOOPS_WARNING),
        "retry_wait CALLER_HINT must warn against loops"
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
