use std::{cell::RefCell, rc::Rc};

use super::*;
use crate::client::SessionEventStore;
use crate::connection::connection_stream::{
    StreamLineBuffers, stream_new_agent_messages_with_tool_output_compaction,
};
use crate::tool_output_compaction::ToolOutputCompactionConfig;
use csa_process::SpoolRotator;

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

fn assert_fixture_secrets_absent(rendered: &str) {
    for secret in [
        "providerfixture12345",
        "githubfixture12345",
        "fixturebearertoken",
        "https://user:pass@example.test/path?token=fixture12345",
        "fixture12345",
    ] {
        assert!(
            !rendered.contains(secret),
            "fixture secret remained in output: {secret}\n{rendered}"
        );
    }
}

#[test]
fn large_tool_output_is_compacted_to_summary_and_sidecar() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sidecar_dir = temp.path().join("tool_outputs");
    let large_output = format!(
        "{}\n{}\n{}\n{}\n{}",
        "HEAD: build started",
        "prefix line\n".repeat(600),
        "MIDDLE: sidecar-only content",
        "suffix line\n".repeat(600),
        "TAIL: build completed"
    );
    let events = shared_events(vec![SessionEvent::ToolCallOutput {
        id: "call-large".to_string(),
        title: Some("cargo test --all".to_string()),
        status: "Completed".to_string(),
        output: large_output.clone(),
    }]);
    let mut processed_event_count = 0usize;
    let spool_path = temp.path().join("output.log");
    let mut spool = open_output_spool_file(
        Some(&spool_path),
        csa_process::DEFAULT_SPOOL_MAX_BYTES,
        csa_process::DEFAULT_SPOOL_KEEP_ROTATED,
    );
    let mut metadata = StreamingMetadata::default();
    let mut stdout_buf = String::new();
    let mut thought_buf = String::new();
    let mut compaction = ToolOutputCompactionConfig::new(sidecar_dir.clone(), 128).into_state();

    stream_new_agent_messages_with_tool_output_compaction(
        &events,
        &mut processed_event_count,
        false,
        &mut spool,
        &mut metadata,
        StreamLineBuffers::new(&mut stdout_buf, &mut thought_buf),
        Some(&mut compaction),
    );
    flush_spool(&mut spool);

    let spool_content = std::fs::read_to_string(&spool_path).expect("read spool");
    assert!(spool_content.contains("[tool:output:compacted]"));
    assert!(spool_content.contains("tool_call_id: call-large"));
    assert!(spool_content.contains("tool: cargo test --all"));
    assert!(spool_content.contains("status: Completed"));
    assert!(spool_content.contains("original_bytes:"));
    assert!(spool_content.contains("original_lines:"));
    assert!(spool_content.contains("sidecar_path: tool_outputs/1000000.raw"));
    assert!(spool_content.contains("HEAD: build started"));
    assert!(spool_content.contains("TAIL: build completed"));
    assert!(!spool_content.contains("MIDDLE: sidecar-only content"));

    let raw = std::fs::read_to_string(sidecar_dir.join("1000000.raw")).expect("read sidecar");
    assert_eq!(raw, large_output);

    let manifest =
        std::fs::read_to_string(sidecar_dir.join("manifest.toml")).expect("read sidecar manifest");
    assert!(manifest.contains("compacted = true"));
    assert!(manifest.contains("tool_call_id = \"call-large\""));
    assert!(manifest.contains("tool_title = \"cargo test --all\""));
    assert!(manifest.contains("status = \"Completed\""));
    assert!(manifest.contains("original_lines = "));
}

#[test]
fn compacted_tool_output_redacts_sidecar_summary_and_manifest_metadata() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sidecar_dir = temp.path().join("tool_outputs");
    let output = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        "SAFE HEAD: diagnostics begin",
        "OPENAI_API_KEY=providerfixture12345",
        "neutral build line\n".repeat(500),
        "GITHUB_TOKEN=githubfixture12345",
        "Authorization: Bearer fixturebearertoken",
        "https://user:pass@example.test/path?token=fixture12345",
        "neutral tail line\n".repeat(500),
        "SAFE TAIL: diagnostics end"
    );
    let events = shared_events(vec![SessionEvent::ToolCallOutput {
        id: "call-secret".to_string(),
        title: Some("curl with GITHUB_TOKEN=githubfixture12345".to_string()),
        status: "Failed".to_string(),
        output,
    }]);
    let mut processed_event_count = 0usize;
    let spool_path = temp.path().join("output.log");
    let mut spool = open_output_spool_file(
        Some(&spool_path),
        csa_process::DEFAULT_SPOOL_MAX_BYTES,
        csa_process::DEFAULT_SPOOL_KEEP_ROTATED,
    );
    let mut metadata = StreamingMetadata::default();
    let mut stdout_buf = String::new();
    let mut thought_buf = String::new();
    let mut compaction = ToolOutputCompactionConfig::new(sidecar_dir.clone(), 128).into_state();

    stream_new_agent_messages_with_tool_output_compaction(
        &events,
        &mut processed_event_count,
        false,
        &mut spool,
        &mut metadata,
        StreamLineBuffers::new(&mut stdout_buf, &mut thought_buf),
        Some(&mut compaction),
    );
    flush_spool(&mut spool);

    let spool_content = std::fs::read_to_string(&spool_path).expect("read spool");
    assert!(spool_content.contains("[tool:output:compacted]"));
    assert!(spool_content.contains("SAFE HEAD: diagnostics begin"));
    assert!(spool_content.contains("SAFE TAIL: diagnostics end"));
    assert!(spool_content.contains("[REDACTED]"));
    assert_fixture_secrets_absent(&spool_content);

    let sidecar = std::fs::read_to_string(sidecar_dir.join("1000000.raw")).expect("read sidecar");
    assert!(sidecar.contains("SAFE HEAD: diagnostics begin"));
    assert!(sidecar.contains("SAFE TAIL: diagnostics end"));
    assert!(sidecar.contains("[REDACTED]"));
    assert_fixture_secrets_absent(&sidecar);

    let manifest =
        std::fs::read_to_string(sidecar_dir.join("manifest.toml")).expect("read sidecar manifest");
    assert!(manifest.contains("compacted = true"));
    assert!(manifest.contains("tool_call_id = \"call-secret\""));
    assert!(manifest.contains("tool_title = \"curl with [REDACTED]\""));
    assert_fixture_secrets_absent(&manifest);
}

#[test]
fn short_tool_output_is_not_compacted_or_sidecarred() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sidecar_dir = temp.path().join("tool_outputs");
    let output = "short stdout\nshort stderr\n".to_string();
    let events = shared_events(vec![SessionEvent::ToolCallOutput {
        id: "call-short".to_string(),
        title: Some("echo short".to_string()),
        status: "Completed".to_string(),
        output: output.clone(),
    }]);
    let mut processed_event_count = 0usize;
    let spool_path = temp.path().join("output.log");
    let mut spool = open_output_spool_file(
        Some(&spool_path),
        csa_process::DEFAULT_SPOOL_MAX_BYTES,
        csa_process::DEFAULT_SPOOL_KEEP_ROTATED,
    );
    let mut metadata = StreamingMetadata::default();
    let mut stdout_buf = String::new();
    let mut thought_buf = String::new();
    let mut compaction = ToolOutputCompactionConfig::new(sidecar_dir.clone(), 1024).into_state();

    stream_new_agent_messages_with_tool_output_compaction(
        &events,
        &mut processed_event_count,
        false,
        &mut spool,
        &mut metadata,
        StreamLineBuffers::new(&mut stdout_buf, &mut thought_buf),
        Some(&mut compaction),
    );
    flush_spool(&mut spool);

    let spool_content = std::fs::read_to_string(&spool_path).expect("read spool");
    assert!(spool_content.contains("[tool:output] call-short Completed"));
    assert!(spool_content.contains(&output));
    assert!(!spool_content.contains("[tool:output:compacted]"));
    assert!(!sidecar_dir.exists());
}

#[test]
fn failing_tool_output_summary_preserves_tail_diagnostics() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sidecar_dir = temp.path().join("tool_outputs");
    let failure_tail = "FAILED: test panic at src/lib.rs:42";
    let output = format!("{}\n{failure_tail}\n", "normal line\n".repeat(128));
    let events = shared_events(vec![SessionEvent::ToolCallOutput {
        id: "call-fail".to_string(),
        title: Some("cargo test failing_case".to_string()),
        status: "Failed".to_string(),
        output,
    }]);
    let mut processed_event_count = 0usize;
    let spool_path = temp.path().join("output.log");
    let mut spool = open_output_spool_file(
        Some(&spool_path),
        csa_process::DEFAULT_SPOOL_MAX_BYTES,
        csa_process::DEFAULT_SPOOL_KEEP_ROTATED,
    );
    let mut metadata = StreamingMetadata::default();
    let mut stdout_buf = String::new();
    let mut thought_buf = String::new();
    let mut compaction = ToolOutputCompactionConfig::new(sidecar_dir, 128).into_state();

    stream_new_agent_messages_with_tool_output_compaction(
        &events,
        &mut processed_event_count,
        false,
        &mut spool,
        &mut metadata,
        StreamLineBuffers::new(&mut stdout_buf, &mut thought_buf),
        Some(&mut compaction),
    );
    flush_spool(&mut spool);

    let spool_content = std::fs::read_to_string(&spool_path).expect("read spool");
    assert!(spool_content.contains("[tool:output:compacted]"));
    assert!(spool_content.contains("status: Failed"));
    assert!(
        spool_content.contains(failure_tail),
        "bounded summary must preserve failing tail diagnostics: {spool_content}"
    );
}
