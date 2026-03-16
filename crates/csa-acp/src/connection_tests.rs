use super::*;
use std::sync::{LazyLock, Mutex};

use crate::client::{MAX_RETAINED_EVENTS, SessionEventStore};

fn flush_spool(spool: &mut Option<SpoolRotator>) {
    if let Some(w) = spool {
        w.flush().expect("flush spool");
    }
}

fn shared_events(retained: Vec<SessionEvent>) -> SharedEvents {
    let mut store = SessionEventStore::default();
    for event in retained {
        store.push(event);
    }
    Rc::new(RefCell::new(store))
}

static HEARTBEAT_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn restore_env_var(key: &str, original: Option<String>) {
    // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
    unsafe {
        match original {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
}

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
fn test_collect_agent_output_includes_thoughts() {
    let events = shared_events(vec![
        SessionEvent::AgentMessage("Hello".to_string()),
        SessionEvent::AgentThought("Thinking...".to_string()),
        SessionEvent::AgentMessage(" world".to_string()),
    ]);
    let mut index = 0;
    let mut spool: Option<SpoolRotator> = None;
    let mut metadata = StreamingMetadata::default();

    stream_new_agent_messages(&events, &mut index, false, &mut spool, &mut metadata);
    let output = collect_agent_output(&metadata);
    assert_eq!(output, "HelloThinking... world");
}

#[test]
fn stream_new_agent_messages_includes_thoughts_in_spool() {
    let events = Rc::new(RefCell::new(SessionEventStore::default()));
    events
        .borrow_mut()
        .push(SessionEvent::AgentMessage("hello".to_string()));
    events
        .borrow_mut()
        .push(SessionEvent::AgentThought("...thinking".to_string()));

    let temp = tempfile::tempdir().expect("tempdir");
    let spool_path = temp.path().join("output.log");
    let mut spool = open_output_spool_file(
        Some(&spool_path),
        DEFAULT_SPOOL_MAX_BYTES,
        DEFAULT_SPOOL_KEEP_ROTATED,
    );
    let mut index = 0;
    let mut metadata = StreamingMetadata::default();

    stream_new_agent_messages(&events, &mut index, false, &mut spool, &mut metadata);
    flush_spool(&mut spool);
    assert_eq!(
        std::fs::read_to_string(&spool_path).expect("read spool"),
        "hello...thinking"
    );
    // Retained events stay in memory for downstream consumers; index advances.
    assert_eq!(index, 2);
    assert_eq!(events.borrow().len(), 2, "events must NOT be drained");
    assert_eq!(metadata.total_events_count, 2);
}

#[test]
fn stream_new_agent_messages_writes_spool_incrementally() {
    let events = Rc::new(RefCell::new(SessionEventStore::default()));
    events
        .borrow_mut()
        .push(SessionEvent::AgentMessage("hello".to_string()));

    let temp = tempfile::tempdir().expect("tempdir");
    let spool_path = temp.path().join("output.log");
    let mut spool = open_output_spool_file(
        Some(&spool_path),
        DEFAULT_SPOOL_MAX_BYTES,
        DEFAULT_SPOOL_KEEP_ROTATED,
    );
    let mut index = 0;
    let mut metadata = StreamingMetadata::default();

    stream_new_agent_messages(&events, &mut index, false, &mut spool, &mut metadata);
    flush_spool(&mut spool);
    assert_eq!(
        std::fs::read_to_string(&spool_path).expect("read spool"),
        "hello"
    );
    assert_eq!(index, 1);
    assert_eq!(events.borrow().len(), 1);

    events
        .borrow_mut()
        .push(SessionEvent::AgentMessage(" world".to_string()));
    stream_new_agent_messages(&events, &mut index, false, &mut spool, &mut metadata);
    flush_spool(&mut spool);
    assert_eq!(
        std::fs::read_to_string(&spool_path).expect("read spool"),
        "hello world"
    );
    assert_eq!(index, 2);
    assert_eq!(events.borrow().len(), 2);
    assert_eq!(metadata.total_events_count, 2);
}

#[test]
fn stream_new_agent_messages_skips_non_message_events() {
    let events = shared_events(vec![
        SessionEvent::Other("x".to_string()),
        SessionEvent::ToolCallCompleted {
            id: "1".to_string(),
            status: "completed".to_string(),
        },
    ]);
    let mut index = 0;
    let mut spool: Option<SpoolRotator> = None;
    let mut metadata = StreamingMetadata::default();

    stream_new_agent_messages(&events, &mut index, false, &mut spool, &mut metadata);
    assert_eq!(index, 2);
    assert_eq!(events.borrow().len(), 2);
    assert_eq!(metadata.total_events_count, 2);
}

#[test]
fn collect_agent_output_excludes_diagnostic_events() {
    let events = shared_events(vec![
        SessionEvent::AgentMessage("Hello".to_string()),
        SessionEvent::PlanUpdate("step 1".to_string()),
        SessionEvent::ToolCallStarted {
            id: "t1".to_string(),
            title: "Read".to_string(),
            kind: "tool_use".to_string(),
        },
        SessionEvent::AgentThought("hmm".to_string()),
        SessionEvent::ToolCallCompleted {
            id: "t1".to_string(),
            status: "completed".to_string(),
        },
        SessionEvent::Other("misc".to_string()),
        SessionEvent::AgentMessage(" world".to_string()),
    ]);
    let mut index = 0;
    let mut spool: Option<SpoolRotator> = None;
    let mut metadata = StreamingMetadata::default();

    stream_new_agent_messages(&events, &mut index, false, &mut spool, &mut metadata);
    let output = collect_agent_output(&metadata);
    assert_eq!(
        output, "Hellohmm world",
        "collect_agent_output must only include AgentMessage and AgentThought"
    );
    assert!(metadata.has_tool_calls);
    assert!(metadata.has_plan_updates);
    assert_eq!(metadata.total_events_count, 7);
}

#[test]
fn stream_new_agent_messages_writes_all_event_types_to_spool() {
    let events = shared_events(vec![
        SessionEvent::AgentMessage("msg".to_string()),
        SessionEvent::PlanUpdate("plan step".to_string()),
        SessionEvent::ToolCallStarted {
            id: "t1".to_string(),
            title: "Edit".to_string(),
            kind: "tool_use".to_string(),
        },
        SessionEvent::ToolCallCompleted {
            id: "t1".to_string(),
            status: "done".to_string(),
        },
        SessionEvent::Other("extra".to_string()),
        SessionEvent::AgentThought("thought".to_string()),
    ]);

    let temp = tempfile::tempdir().expect("tempdir");
    let spool_path = temp.path().join("output.log");
    let mut spool = open_output_spool_file(
        Some(&spool_path),
        DEFAULT_SPOOL_MAX_BYTES,
        DEFAULT_SPOOL_KEEP_ROTATED,
    );
    let mut index = 0;
    let mut metadata = StreamingMetadata::default();

    stream_new_agent_messages(&events, &mut index, false, &mut spool, &mut metadata);
    flush_spool(&mut spool);
    let spool_content = std::fs::read_to_string(&spool_path).expect("read spool");
    assert!(
        spool_content.contains("msg"),
        "spool must include AgentMessage"
    );
    assert!(
        spool_content.contains("[plan] plan step"),
        "spool must include PlanUpdate"
    );
    assert!(
        spool_content.contains("[tool:started] Edit"),
        "spool must include ToolCallStarted"
    );
    assert!(
        spool_content.contains("[tool:completed] done"),
        "spool must include ToolCallCompleted"
    );
    assert!(
        spool_content.contains("[other] extra"),
        "spool must include Other"
    );
    assert!(
        spool_content.contains("thought"),
        "spool must include AgentThought"
    );
    assert_eq!(index, 6);
    assert_eq!(events.borrow().len(), 6);
    assert_eq!(metadata.total_events_count, 6);
    assert!(metadata.has_tool_calls);
    assert!(metadata.has_plan_updates);
}

#[test]
fn stream_preserves_events_for_downstream_consumers() {
    let events = Rc::new(RefCell::new(SessionEventStore::default()));
    let mut index = 0;
    let mut spool: Option<SpoolRotator> = None;
    let mut metadata = StreamingMetadata::default();

    // Push 10000 events and stream them in batches.
    // Retained events stay bounded in memory while the total count tracks all
    // events seen across the full prompt turn.
    for i in 0..10_000 {
        events
            .borrow_mut()
            .push(SessionEvent::AgentMessage(format!("msg-{i}\n")));
        if i % 100 == 99 {
            stream_new_agent_messages(&events, &mut index, false, &mut spool, &mut metadata);
            // Index advances to match the number of events seen so far.
            assert_eq!(index, i + 1, "processed_index must track event count");
        }
    }
    stream_new_agent_messages(&events, &mut index, false, &mut spool, &mut metadata);
    let retained = events.borrow().events();
    assert_eq!(retained.len(), MAX_RETAINED_EVENTS);
    match retained.first() {
        Some(SessionEvent::AgentMessage(text)) => assert_eq!(text, "msg-8000\n"),
        other => panic!("unexpected first retained event: {other:?}"),
    }
    assert_eq!(index, 10_000);
    assert_eq!(metadata.total_events_count, 10_000);
    // Tail buffer is bounded by TAIL_BUFFER_MAX_BYTES (1 MiB), not unbounded.
    assert!(
        metadata.tail_text.len() <= 1024 * 1024 + 64,
        "tail_text must be bounded by TAIL_BUFFER_MAX_BYTES"
    );
}

#[test]
fn spool_writes_all_data_without_truncation() {
    let events = Rc::new(RefCell::new(SessionEventStore::default()));
    let temp = tempfile::tempdir().expect("tempdir");
    let spool_path = temp.path().join("output.log");
    let mut spool = open_output_spool_file(
        Some(&spool_path),
        DEFAULT_SPOOL_MAX_BYTES,
        DEFAULT_SPOOL_KEEP_ROTATED,
    );
    let mut index = 0;
    let mut metadata = StreamingMetadata::default();

    // Write several chunks and verify none are truncated.
    let chunk_size = 1024; // 1 KB per event
    let chunk = "x".repeat(chunk_size);
    let num_chunks = 100;
    for _ in 0..num_chunks {
        events
            .borrow_mut()
            .push(SessionEvent::AgentMessage(chunk.clone()));
        stream_new_agent_messages(&events, &mut index, false, &mut spool, &mut metadata);
    }
    flush_spool(&mut spool);

    let file_size = std::fs::metadata(&spool_path)
        .expect("spool file exists")
        .len() as usize;
    assert_eq!(
        file_size,
        chunk_size * num_chunks,
        "spool must write all data without truncation (preserves tail markers)"
    );
    assert_eq!(metadata.total_events_count, num_chunks);
    assert_eq!(
        metadata.spool_bytes_written,
        (chunk_size * num_chunks) as u64
    );
}

#[test]
fn heartbeat_interval_defaults_to_enabled() {
    let _env_lock = HEARTBEAT_ENV_LOCK
        .lock()
        .expect("heartbeat env lock poisoned");
    let original = std::env::var(HEARTBEAT_INTERVAL_ENV).ok();
    // SAFETY: test-scoped env mutation, restored immediately.
    unsafe { std::env::remove_var(HEARTBEAT_INTERVAL_ENV) };
    let resolved = resolve_heartbeat_interval();
    restore_env_var(HEARTBEAT_INTERVAL_ENV, original);
    assert_eq!(resolved, Some(Duration::from_secs(DEFAULT_HEARTBEAT_SECS)));
}

#[test]
fn heartbeat_interval_can_be_disabled_with_zero() {
    let _env_lock = HEARTBEAT_ENV_LOCK
        .lock()
        .expect("heartbeat env lock poisoned");
    let original = std::env::var(HEARTBEAT_INTERVAL_ENV).ok();
    // SAFETY: test-scoped env mutation, restored immediately.
    unsafe { std::env::set_var(HEARTBEAT_INTERVAL_ENV, "0") };
    let resolved = resolve_heartbeat_interval();
    restore_env_var(HEARTBEAT_INTERVAL_ENV, original);
    assert_eq!(resolved, None);
}
