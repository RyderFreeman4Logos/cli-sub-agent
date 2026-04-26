//! Tests asserting `CSA:CALLER_HINT` emissions carry the no-stack-wakeup
//! warning at every emit site.
//!
//! The warning is a stable contract callers depend on (AGENTS.md rules 042 +
//! 046, GitHub issue #1132). Source-level assertions guard against accidental
//! removal during future edits to the four daemon entry points.
#![cfg(test)]

const NO_STACK_WAKEUP_WARNING: &str = "do NOT stack ScheduleWakeup, /loop, or sleep loops on top";

const RUN_CMD_DAEMON_SRC: &str = include_str!("run_cmd_daemon.rs");
const PLAN_CMD_DAEMON_SRC: &str = include_str!("plan_cmd_daemon.rs");
const SESSION_CMDS_DAEMON_WAIT_SRC: &str = include_str!("session_cmds_daemon_wait.rs");

fn caller_hint_blocks(src: &str) -> Vec<&str> {
    src.split("CSA:CALLER_HINT")
        .skip(1)
        .map(|tail| {
            let end = tail.find("-->").unwrap_or(tail.len());
            &tail[..end]
        })
        .collect()
}

#[test]
fn run_cmd_daemon_wait_hint_warns_no_stack_wakeup() {
    let blocks = caller_hint_blocks(RUN_CMD_DAEMON_SRC);
    assert_eq!(blocks.len(), 1, "run_cmd_daemon emits one CALLER_HINT");
    assert!(
        blocks[0].contains("action=\\\"wait\\\""),
        "run_cmd_daemon emits action=\"wait\""
    );
    assert!(
        blocks[0].contains(NO_STACK_WAKEUP_WARNING),
        "run_cmd_daemon CALLER_HINT must warn against ScheduleWakeup/loop stacking; missing warning: {NO_STACK_WAKEUP_WARNING}"
    );
}

#[test]
fn plan_cmd_daemon_wait_hint_warns_no_stack_wakeup() {
    let blocks = caller_hint_blocks(PLAN_CMD_DAEMON_SRC);
    assert_eq!(blocks.len(), 1, "plan_cmd_daemon emits one CALLER_HINT");
    assert!(
        blocks[0].contains("action=\\\"wait\\\""),
        "plan_cmd_daemon emits action=\"wait\""
    );
    assert!(
        blocks[0].contains(NO_STACK_WAKEUP_WARNING),
        "plan_cmd_daemon CALLER_HINT must warn against ScheduleWakeup/loop stacking"
    );
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
