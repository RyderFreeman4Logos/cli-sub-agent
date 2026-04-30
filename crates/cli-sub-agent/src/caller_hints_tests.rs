//! Tests asserting `CSA:CALLER_HINT` emissions carry the no-stack-wakeup
//! warning at every emit site.
//!
//! The warning is a stable contract callers depend on (AGENTS.md rules 042 +
//! 046, GitHub issue #1132). Source-level assertions guard against accidental
//! removal during future edits to the four daemon entry points.
#![cfg(test)]

const NO_STACK_WAKEUP_WARNING: &str = "do NOT stack ScheduleWakeup, /loop, or sleep loops on top";
const BACKGROUND_WAIT_RECOMMENDATION: &str = "with run_in_background: true";
const TASK_NOTIFICATION_WAKE_SIGNAL: &str = "The task-notification IS your wake signal";

const RUN_CMD_DAEMON_SRC: &str = include_str!("run_cmd_daemon.rs");
const PLAN_CMD_DAEMON_SRC: &str = include_str!("plan_cmd_daemon.rs");
const SESSION_CMDS_DAEMON_WAIT_SRC: &str = include_str!("session_cmds_daemon_wait.rs");

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
        block.contains(BACKGROUND_WAIT_RECOMMENDATION),
        "{site} CALLER_HINT must recommend backgrounding session wait with run_in_background: true"
    );
    assert!(
        block.contains(TASK_NOTIFICATION_WAKE_SIGNAL),
        "{site} CALLER_HINT must state that task-notification is the wake signal"
    );
    assert!(
        block.contains(NO_STACK_WAKEUP_WARNING),
        "{site} CALLER_HINT must warn against ScheduleWakeup/loop stacking; missing warning: {NO_STACK_WAKEUP_WARNING}"
    );
    assert!(
        !block.contains("in a SEPARATE Bash call"),
        "{site} CALLER_HINT must not lead with the old foreground-wait phrasing"
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
                "<!-- CSA:CALLER_HINT action=\"wait\" \
                 rule=\"do NOT stack ScheduleWakeup, /loop, or sleep loops on top\" -->"
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
fn run_cmd_daemon_wait_hint_warns_no_stack_wakeup() {
    let blocks = caller_hint_blocks(RUN_CMD_DAEMON_SRC);
    assert_eq!(blocks.len(), 1, "run_cmd_daemon emits one CALLER_HINT");
    assert_wait_hint_contract(blocks[0], "run_cmd_daemon");
}

#[test]
fn plan_cmd_daemon_wait_hint_warns_no_stack_wakeup() {
    let blocks = caller_hint_blocks(PLAN_CMD_DAEMON_SRC);
    assert_eq!(blocks.len(), 1, "plan_cmd_daemon emits one CALLER_HINT");
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
    let retry = blocks
        .iter()
        .find(|b| b.contains("action=\\\"retry_wait\\\""))
        .expect("retry_wait CALLER_HINT block present");
    assert!(
        retry.contains(NO_STACK_WAKEUP_WARNING),
        "retry_wait CALLER_HINT must warn against ScheduleWakeup/loop stacking"
    );
}

#[test]
fn session_cmds_daemon_wait_next_session_hint_warns_no_stack_wakeup() {
    let blocks = caller_hint_blocks(SESSION_CMDS_DAEMON_WAIT_SRC);
    let next = blocks
        .iter()
        .find(|b| b.contains("action=\\\"next_session\\\""))
        .expect("next_session CALLER_HINT block present");
    assert!(
        next.contains(NO_STACK_WAKEUP_WARNING),
        "next_session CALLER_HINT must warn against ScheduleWakeup/loop stacking"
    );
}
