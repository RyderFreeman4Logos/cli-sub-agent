use super::*;

#[test]
fn stripped_env_vars_contains_claudecode() {
    assert!(
        AcpConnection::STRIPPED_ENV_VARS.contains(&"CLAUDECODE"),
        "STRIPPED_ENV_VARS must strip CLAUDECODE (recursion detection)"
    );
    assert!(
        AcpConnection::STRIPPED_ENV_VARS.contains(&"CLAUDE_CODE_ENTRYPOINT"),
        "STRIPPED_ENV_VARS must strip CLAUDE_CODE_ENTRYPOINT (parent context)"
    );
}

#[test]
fn format_stderr_empty() {
    assert_eq!(AcpConnection::format_stderr(""), String::new());
}

#[test]
fn format_stderr_whitespace_only() {
    assert_eq!(AcpConnection::format_stderr("  \n  "), String::new());
}

#[test]
fn format_stderr_with_content() {
    assert_eq!(
        AcpConnection::format_stderr("  some error\n"),
        "; stderr: some error"
    );
}

/// Verify that `env_remove` with `STRIPPED_ENV_VARS` actually prevents
/// a child process from seeing `CLAUDECODE`.
///
/// This test validates the *mechanism* (env_remove + var list), not the
/// private `build_cmd_base` method directly (tokio::Command doesn't
/// expose env introspection).  Since `build_cmd_base` and the cgroup
/// path both iterate `STRIPPED_ENV_VARS` with `cmd.env_remove(var)`,
/// verifying the var list and the env_remove effect is sufficient.
///
/// Note: uses `unsafe set_var/remove_var` which is unsound under
/// parallel test execution.  Acceptable here because the test is
/// short-lived and the vars are cleaned up immediately.
#[tokio::test]
async fn env_remove_strips_claudecode_from_child() {
    // Save original values so we can restore after the test.
    let orig_claudecode = std::env::var("CLAUDECODE").ok();
    let orig_entrypoint = std::env::var("CLAUDE_CODE_ENTRYPOINT").ok();

    // SAFETY: set_var is unsound under parallel test execution (Rust
    // 1.66+ deprecation).  Acceptable here: this test is short-lived,
    // single-threaded (#[tokio::test] default), and we restore the
    // original value immediately after spawning the child.
    unsafe { std::env::set_var("CLAUDECODE", "1") };

    let mut std_cmd = std::process::Command::new("printenv");
    std_cmd.current_dir(std::env::current_dir().unwrap());
    for var in AcpConnection::STRIPPED_ENV_VARS {
        std_cmd.env_remove(var);
    }

    let output = std_cmd.output().expect("printenv should be available");
    let stdout = String::from_utf8_lossy(&output.stdout);

    // SAFETY: restore original env state (same single-threaded context).
    unsafe {
        match orig_claudecode {
            Some(v) => std::env::set_var("CLAUDECODE", v),
            None => std::env::remove_var("CLAUDECODE"),
        }
        match orig_entrypoint {
            Some(v) => std::env::set_var("CLAUDE_CODE_ENTRYPOINT", v),
            None => std::env::remove_var("CLAUDE_CODE_ENTRYPOINT"),
        }
    }

    assert!(
        !stdout.lines().any(|line| line.starts_with("CLAUDECODE=")),
        "CLAUDECODE should have been stripped from child environment, got:\n{stdout}"
    );
    assert!(
        !stdout
            .lines()
            .any(|line| line.starts_with("CLAUDE_CODE_ENTRYPOINT=")),
        "CLAUDE_CODE_ENTRYPOINT should have been stripped"
    );
}

#[test]
fn stream_new_agent_messages_writes_spool_incrementally() {
    let events = Rc::new(RefCell::new(Vec::new()));
    events
        .borrow_mut()
        .push(SessionEvent::AgentMessage("hello".to_string()));

    let temp = tempfile::tempdir().expect("tempdir");
    let spool_path = temp.path().join("output.log");
    let mut spool = open_output_spool_file(Some(&spool_path));
    let mut index = 0;

    stream_new_agent_messages(&events, &mut index, false, &mut spool);
    assert_eq!(
        std::fs::read_to_string(&spool_path).expect("read spool"),
        "hello"
    );
    assert_eq!(index, 1);

    events
        .borrow_mut()
        .push(SessionEvent::AgentMessage(" world".to_string()));
    stream_new_agent_messages(&events, &mut index, false, &mut spool);
    assert_eq!(
        std::fs::read_to_string(&spool_path).expect("read spool"),
        "hello world"
    );
    assert_eq!(index, 2);
}

#[test]
fn stream_new_agent_messages_skips_non_message_events() {
    let events = Rc::new(RefCell::new(vec![
        SessionEvent::Other("x".to_string()),
        SessionEvent::ToolCallCompleted {
            id: "1".to_string(),
            status: "completed".to_string(),
        },
    ]));
    let mut index = 0;
    let mut spool = None;

    stream_new_agent_messages(&events, &mut index, false, &mut spool);
    assert_eq!(index, 2);
}
