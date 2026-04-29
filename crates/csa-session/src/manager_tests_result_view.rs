use std::io;
use std::sync::{Arc, LazyLock, Mutex};
use tracing_subscriber::fmt::MakeWriter;

static TEST_TRACING_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(Clone, Default)]
struct SharedLogBuffer {
    inner: Arc<Mutex<Vec<u8>>>,
}

impl SharedLogBuffer {
    fn contents(&self) -> String {
        String::from_utf8(self.inner.lock().expect("log buffer poisoned").clone())
            .expect("log buffer should be valid UTF-8")
    }
}

impl<'a> MakeWriter<'a> for SharedLogBuffer {
    type Writer = SharedLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        SharedLogWriter {
            inner: Arc::clone(&self.inner),
        }
    }
}

struct SharedLogWriter {
    inner: Arc<Mutex<Vec<u8>>>,
}

impl io::Write for SharedLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner
            .lock()
            .expect("log buffer poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn test_load_result_view_ignores_orphaned_manager_sidecar() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();
    let session_dir = get_session_dir_in(td.path(), &state.meta_session_id);

    let now = chrono::Utc::now();
    let runtime_result = crate::result::SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "runtime summary".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 1,
        artifacts: vec![crate::result::SessionArtifact::new("output/acp-events.jsonl")],
        peak_memory_mb: None,
            manager_fields: Default::default(),
    };
    save_result_in(
        td.path(),
        &state.meta_session_id,
        &runtime_result,
        crate::SaveOptions::default(),
    )
    .unwrap();

    std::fs::write(
        session_dir.join(manager_result::CONTRACT_RESULT_ARTIFACT_PATH),
        "[report]\nsummary = \"manager-visible\"\n",
    )
    .unwrap();
    std::fs::write(
        session_dir.join(manager_result::LEGACY_USER_RESULT_ARTIFACT_PATH),
        "[artifacts]\ncount = 2\n",
    )
    .unwrap();

    let loaded = load_result_view_in(td.path(), &state.meta_session_id)
        .unwrap()
        .expect("result view should exist");
    assert_eq!(loaded.envelope.summary, "runtime summary");
    assert!(loaded.manager_sidecar.is_none());
    assert_eq!(
        loaded.legacy_sidecar.as_ref().and_then(|value| value.get("artifacts")),
        Some(&toml::Value::Table(
            [("count".to_string(), toml::Value::Integer(2))]
                .into_iter()
                .collect()
        ))
    );
}

#[test]
fn test_load_result_merges_manager_sidecar_sections_into_runtime_result() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();
    let session_dir = get_session_dir_in(td.path(), &state.meta_session_id);

    let now = chrono::Utc::now();
    let runtime_result = crate::result::SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "runtime summary".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 1,
        artifacts: vec![crate::result::SessionArtifact::new("output/acp-events.jsonl")],
        peak_memory_mb: None,
        manager_fields: Default::default(),
    };
    std::fs::write(
        session_dir.join(manager_result::CONTRACT_RESULT_ARTIFACT_PATH),
        r#"
[result]
done = true

[report]
summary = "manager-visible"

[timing]
started_at = "2026-02-11T10:00:00Z"
ended_at = "2026-02-11T10:05:00Z"

[tool]
name = "claude-code"

[review]
author_tool = "claude-code"
reviewer_tool = "codex"

[clarification]
blocking_reason = "Need scope confirmation"
questions = ["Question 1", "Question 2"]

[artifacts]
count = 2
"#,
    )
    .unwrap();
    save_result_in(
        td.path(),
        &state.meta_session_id,
        &runtime_result,
        crate::SaveOptions::default(),
    )
    .unwrap();

    let loaded = load_result_in(td.path(), &state.meta_session_id)
        .unwrap()
        .expect("result should exist");

    assert_eq!(loaded.summary, "runtime summary");
    assert_eq!(
        loaded.manager_fields.result.as_ref().and_then(|value| value.get("done")),
        Some(&toml::Value::Boolean(true))
    );
    assert_eq!(
        loaded
            .manager_fields
            .report
            .as_ref()
            .and_then(|value| value.get("summary")),
        Some(&toml::Value::String("manager-visible".to_string()))
    );
    assert_eq!(
        loaded
            .manager_fields
            .timing
            .as_ref()
            .and_then(|value| value.get("started_at")),
        Some(&toml::Value::String("2026-02-11T10:00:00Z".to_string()))
    );
    assert_eq!(
        loaded
            .manager_fields
            .tool
            .as_ref()
            .and_then(|value| value.get("name")),
        Some(&toml::Value::String("claude-code".to_string()))
    );
    assert_eq!(
        loaded
            .manager_fields
            .review
            .as_ref()
            .and_then(|value| value.get("reviewer_tool")),
        Some(&toml::Value::String("codex".to_string()))
    );
    assert_eq!(
        loaded
            .manager_fields
            .clarification
            .as_ref()
            .and_then(|value| value.get("blocking_reason")),
        Some(&toml::Value::String("Need scope confirmation".to_string()))
    );
    assert_eq!(
        loaded
            .manager_fields
            .artifacts
            .as_ref()
            .and_then(|value| value.get("count")),
        Some(&toml::Value::Integer(2))
    );
}

#[test]
fn test_manager_sidecar_roundtrip_preserves_full_sa_schema() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();
    let session_dir = get_session_dir_in(td.path(), &state.meta_session_id);

    let now = chrono::Utc::now();
    let runtime_result = crate::result::SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "runtime summary".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 1,
        artifacts: vec![crate::result::SessionArtifact::new("output/acp-events.jsonl")],
        peak_memory_mb: None,
        manager_fields: Default::default(),
    };
    let input_sidecar = toml::Value::Table(toml::toml! {
        [result]
        status = "needs_clarification"
        summary = "Need guidance on rollout"
        error_code = ""
        session_id = "019c4c24-full-schema"

        [report]
        what_was_done = "Collected manager-facing report fields"
        key_decisions = ["Kept sidecar load non-fatal", "Preserved contract sections"]
        risks_identified = ["Awaiting user answer"]
        files_changed = 2
        tests_added = 1
        tests_passing = true

        [timing]
        started_at = "2026-02-11T10:00:00Z"
        ended_at = "2026-02-11T10:05:00Z"

        [tool]
        name = "claude-code"

        [review]
        author_tool = "claude-code"
        reviewer_tool = "codex"

        [clarification]
        questions = ["Should token expiry be configurable?", "Should refresh tokens be included in this scope?"]
        blocking_reason = "Security requirement is ambiguous"

        [artifacts]
        todo_path = "output/TODO.md"
        commit_hash = "abc1234"
        review_result = "CLEAN"
    });
    let input_sidecar_str = toml::to_string_pretty(&input_sidecar).unwrap();
    std::fs::write(
        session_dir.join(manager_result::CONTRACT_RESULT_ARTIFACT_PATH),
        &input_sidecar_str,
    )
    .unwrap();
    save_result_in(
        td.path(),
        &state.meta_session_id,
        &runtime_result,
        crate::SaveOptions::default(),
    )
    .unwrap();

    let loaded = load_result_in(td.path(), &state.meta_session_id)
        .unwrap()
        .expect("result should exist");
    assert_eq!(loaded.manager_fields.as_sidecar(), Some(input_sidecar.clone()));
    assert_eq!(
        loaded
            .manager_fields
            .result
            .as_ref()
            .and_then(|value| value.get("status")),
        Some(&toml::Value::String("needs_clarification".to_string()))
    );
    assert_eq!(
        loaded
            .manager_fields
            .report
            .as_ref()
            .and_then(|value| value.get("files_changed")),
        Some(&toml::Value::Integer(2))
    );
    assert_eq!(
        loaded
            .manager_fields
            .timing
            .as_ref()
            .and_then(|value| value.get("ended_at")),
        Some(&toml::Value::String("2026-02-11T10:05:00Z".to_string()))
    );
    assert_eq!(
        loaded
            .manager_fields
            .tool
            .as_ref()
            .and_then(|value| value.get("name")),
        Some(&toml::Value::String("claude-code".to_string()))
    );
    assert_eq!(
        loaded
            .manager_fields
            .review
            .as_ref()
            .and_then(|value| value.get("author_tool")),
        Some(&toml::Value::String("claude-code".to_string()))
    );
    assert_eq!(
        loaded
            .manager_fields
            .clarification
            .as_ref()
            .and_then(|value| value.get("questions"))
            .and_then(toml::Value::as_array)
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(
        loaded
            .manager_fields
            .artifacts
            .as_ref()
            .and_then(|value| value.get("review_result")),
        Some(&toml::Value::String("CLEAN".to_string()))
    );

    let roundtrip_sidecar = loaded
        .manager_fields
        .as_sidecar()
        .expect("sidecar should round-trip");
    let roundtrip_path = session_dir.join("output/result.roundtrip.toml");
    std::fs::write(
        &roundtrip_path,
        toml::to_string_pretty(&roundtrip_sidecar).unwrap(),
    )
    .unwrap();

    let reloaded: toml::Value =
        toml::from_str(&std::fs::read_to_string(&roundtrip_path).unwrap()).unwrap();
    assert_eq!(reloaded, input_sidecar);
}

#[test]
fn test_load_result_without_sidecar_keeps_manager_fields_empty() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();

    let now = chrono::Utc::now();
    let runtime_result = crate::result::SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "runtime summary".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 1,
        artifacts: vec![crate::result::SessionArtifact::new("output/acp-events.jsonl")],
        peak_memory_mb: None,
        manager_fields: Default::default(),
    };
    save_result_in(
        td.path(),
        &state.meta_session_id,
        &runtime_result,
        crate::SaveOptions::default(),
    )
    .unwrap();

    let loaded = load_result_in(td.path(), &state.meta_session_id)
        .unwrap()
        .expect("result should exist");
    assert!(loaded.manager_fields.is_empty());
}

#[test]
fn test_save_result_with_empty_manager_fields_preserves_existing_sidecar() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();
    let session_dir = get_session_dir_in(td.path(), &state.meta_session_id);
    let now = chrono::Utc::now();

    let populated_result = crate::result::SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "runtime summary".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 1,
        artifacts: vec![crate::result::SessionArtifact::new("output/acp-events.jsonl")],
        peak_memory_mb: None,
        manager_fields: crate::result::SessionManagerFields {
            report: Some(
                toml::toml! {
                    files_changed = 1
                    repo_write_audit = "warn"
                }
                .into(),
            ),
            ..Default::default()
        },
    };
    save_result_in(
        td.path(),
        &state.meta_session_id,
        &populated_result,
        crate::SaveOptions::default(),
    )
    .unwrap();
    assert!(
        session_dir
            .join(manager_result::CONTRACT_RESULT_ARTIFACT_PATH)
            .exists()
    );

    let clean_result = crate::result::SessionResult {
        manager_fields: Default::default(),
        ..populated_result.clone()
    };
    save_result_in(
        td.path(),
        &state.meta_session_id,
        &clean_result,
        crate::SaveOptions::default(),
    )
    .unwrap();

    let sidecar_path = session_dir.join(manager_result::CONTRACT_RESULT_ARTIFACT_PATH);
    assert!(sidecar_path.exists(), "existing sidecar must be preserved");

    let loaded = load_result_in(td.path(), &state.meta_session_id)
        .unwrap()
        .expect("result should exist");
    assert_eq!(loaded.manager_fields.as_sidecar(), populated_result.manager_fields.as_sidecar());
    assert!(
        loaded
            .artifacts
            .iter()
            .any(|artifact| artifact.path == manager_result::CONTRACT_RESULT_ARTIFACT_PATH)
    );
}

#[test]
fn test_clear_manager_sidecar_removes_existing_sidecar() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();
    let session_dir = get_session_dir_in(td.path(), &state.meta_session_id);
    let now = chrono::Utc::now();

    let populated_result = crate::result::SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "runtime summary".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 1,
        artifacts: vec![crate::result::SessionArtifact::new("output/acp-events.jsonl")],
        peak_memory_mb: None,
        manager_fields: crate::result::SessionManagerFields {
            report: Some(
                toml::toml! {
                    files_changed = 1
                    repo_write_audit = "warn"
                }
                .into(),
            ),
            ..Default::default()
        },
    };
    save_result_in(
        td.path(),
        &state.meta_session_id,
        &populated_result,
        crate::SaveOptions::default(),
    )
    .unwrap();
    assert!(
        session_dir
            .join(manager_result::CONTRACT_RESULT_ARTIFACT_PATH)
            .exists()
    );

    crate::clear_manager_sidecar(&session_dir).unwrap();

    let sidecar_path = session_dir.join(manager_result::CONTRACT_RESULT_ARTIFACT_PATH);
    assert!(!sidecar_path.exists(), "explicit clear must remove sidecar");
}

#[test]
fn test_load_result_with_malformed_manager_sidecar_is_non_fatal() {
    let _tracing_guard = TEST_TRACING_LOCK.lock().expect("tracing lock poisoned");
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, Some("codex")).unwrap();
    let session_dir = get_session_dir_in(td.path(), &state.meta_session_id);

    let now = chrono::Utc::now();
    let runtime_result = crate::result::SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "runtime summary".to_string(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: now,
        completed_at: now,
        events_count: 1,
        artifacts: vec![crate::result::SessionArtifact::new("output/acp-events.jsonl")],
        peak_memory_mb: None,
        manager_fields: Default::default(),
    };
    std::fs::write(
        session_dir.join(manager_result::CONTRACT_RESULT_ARTIFACT_PATH),
        "not = [valid toml",
    )
    .unwrap();
    save_result_in(
        td.path(),
        &state.meta_session_id,
        &runtime_result,
        crate::SaveOptions::default(),
    )
    .unwrap();

    let buffer = SharedLogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(buffer.clone())
        .without_time()
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let loaded = load_result_in(td.path(), &state.meta_session_id)
        .unwrap()
        .expect("result should exist");

    assert_eq!(loaded.summary, "runtime summary");
    assert!(loaded.manager_fields.is_empty());
    assert!(buffer.contents().contains(
        "sidecar present but unreadable/malformed; ignoring (runtime envelope still loaded)"
    ));
}

#[test]
fn test_redact_result_sidecar_value_masks_secret_fields() {
    let redacted = manager_result::redact_result_sidecar_value(
        &toml::toml! {
            [auth]
            api_key = "hunter2"
            token = "secret-token"
        }
        .into(),
    )
    .expect("redacted sidecar");

    let rendered = toml::to_string_pretty(&redacted).expect("render redacted sidecar");
    assert!(!rendered.contains("hunter2"));
    assert!(!rendered.contains("secret-token"));
    assert!(rendered.contains("[REDACTED]"));
}
