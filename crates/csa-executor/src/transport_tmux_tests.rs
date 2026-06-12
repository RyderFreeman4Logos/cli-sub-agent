use super::jsonl::JsonlEvent;
use super::*;
use serde::Deserialize;
use std::io::Write;
use std::path::Path;
use tempfile::TempDir;

// ── test-only session index helpers (moved from transport_tmux.rs) ──────

#[derive(Deserialize)]
struct SessionsIndex {
    #[serde(default)]
    entries: Vec<SessionsIndexEntry>,
}

#[derive(Deserialize)]
struct SessionsIndexEntry {
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "fullPath")]
    full_path: Option<PathBuf>,
}

fn find_in_sessions_index(index_path: &Path, session_id: &str) -> Option<PathBuf> {
    let content = fs::read_to_string(index_path).ok()?;
    let index: SessionsIndex = serde_json::from_str(&content).ok()?;
    index
        .entries
        .into_iter()
        .find(|e| e.session_id == session_id)
        .and_then(|e| e.full_path)
}

fn find_jsonl_path(claude_root: &Path, session_id: &str) -> Option<PathBuf> {
    let projects = claude_root.join("projects");
    if !projects.exists() {
        return None;
    }

    let needle = format!("{session_id}.jsonl");
    let Ok(project_dirs) = fs::read_dir(&projects) else {
        return None;
    };

    for entry in project_dirs.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let index_path = path.join("sessions-index.json");
        if index_path.exists()
            && let Some(jsonl) = find_in_sessions_index(&index_path, session_id)
            && jsonl.exists()
        {
            return Some(jsonl);
        }

        let candidate = path.join(&needle);
        if candidate.exists() {
            return Some(candidate);
        }
    }

    None
}

fn write_jsonl(dir: &Path, name: &str, lines: &[&str]) -> PathBuf {
    let path = dir.join(name);
    let mut f = fs::File::create(&path).unwrap();
    for line in lines {
        writeln!(f, "{line}").unwrap();
    }
    path
}

// ── parse_jsonl_line ─────────────────────────────────────────────────────

#[test]
fn parses_turn_duration() {
    let line = r#"{"type":"system","subtype":"turn_duration","durationMs":1234}"#;
    assert!(matches!(
        parse_jsonl_line(line),
        Some(JsonlEvent::TurnDuration)
    ));
}

#[test]
fn parses_compact_boundary() {
    let line = r#"{"type":"system","subtype":"compact_boundary"}"#;
    assert!(matches!(
        parse_jsonl_line(line),
        Some(JsonlEvent::CompactBoundary)
    ));
}

#[test]
fn parses_assistant_text() {
    let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello"}]}}"#;
    match parse_jsonl_line(line) {
        Some(JsonlEvent::AssistantText(t)) => assert_eq!(t, "hello"),
        other => panic!("expected AssistantText, got {other:?}"),
    }
}

#[test]
fn parses_assistant_multiple_text_blocks() {
    let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"foo"},{"type":"text","text":"bar"}]}}"#;
    match parse_jsonl_line(line) {
        Some(JsonlEvent::AssistantText(t)) => assert_eq!(t, "foobar"),
        other => panic!("expected AssistantText, got {other:?}"),
    }
}

#[test]
fn parses_result_event_text() {
    let line = r#"{"type":"result","result":"final answer","session_id":"abc123"}"#;
    match parse_jsonl_line(line) {
        Some(JsonlEvent::ResultText(t)) => assert_eq!(t, "final answer"),
        other => panic!("expected ResultText, got {other:?}"),
    }
}

#[test]
fn result_event_empty_text_returns_none() {
    let line = r#"{"type":"result","result":"","session_id":"abc123"}"#;
    assert!(parse_jsonl_line(line).is_none());
}

#[test]
fn result_event_no_result_field_returns_none() {
    let line = r#"{"type":"result","session_id":"abc123"}"#;
    assert!(parse_jsonl_line(line).is_none());
}

#[test]
fn ignores_unknown_event_type() {
    let line = r#"{"type":"human","message":{"content":"hello"}}"#;
    assert!(parse_jsonl_line(line).is_none());
}

// ── find_jsonl_path ──────────────────────────────────────────────────────

#[test]
fn find_jsonl_by_filename() {
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects").join("my-project");
    fs::create_dir_all(&projects).unwrap();
    let session_id = "abc123";
    let jsonl = projects.join(format!("{session_id}.jsonl"));
    fs::write(&jsonl, b"").unwrap();

    let found = find_jsonl_path(tmp.path(), session_id);
    assert_eq!(found, Some(jsonl));
}

#[test]
fn find_jsonl_via_sessions_index() {
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("projects").join("indexed-project");
    fs::create_dir_all(&projects).unwrap();
    let session_id = "def456";
    let jsonl = projects.join("renamed.jsonl");
    fs::write(&jsonl, b"").unwrap();

    let index = projects.join("sessions-index.json");
    let index_content = format!(
        r#"{{"entries":[{{"sessionId":"{session_id}","fullPath":"{path}"}}]}}"#,
        path = jsonl.display()
    );
    fs::write(&index, index_content).unwrap();

    let found = find_jsonl_path(tmp.path(), session_id);
    assert_eq!(found, Some(jsonl));
}

#[test]
fn find_jsonl_returns_none_when_missing() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("projects")).unwrap();
    assert!(find_jsonl_path(tmp.path(), "nosuchsession").is_none());
}

// ── validate_jsonl_schema ────────────────────────────────────────────────

#[test]
fn schema_validation_passes_for_valid_events() {
    let tmp = TempDir::new().unwrap();
    let path = write_jsonl(
        tmp.path(),
        "session.jsonl",
        &[
            r#"{"type":"system","subtype":"init","sessionId":"s1","timestamp":"2026-01-01"}"#,
            r#"{"type":"human","message":{},"sessionId":"s1","timestamp":"2026-01-01"}"#,
        ],
    );
    assert!(validate_jsonl_schema(&path).is_ok());
}

#[test]
fn schema_validation_fails_when_no_type_field() {
    let tmp = TempDir::new().unwrap();
    let path = write_jsonl(
        tmp.path(),
        "bad.jsonl",
        &[r#"{"foo":"bar","sessionId":"s1"}"#],
    );
    assert!(validate_jsonl_schema(&path).is_err());
}

// ── watch_jsonl_for_turn (sync simulation) ───────────────────────────────

#[tokio::test]
async fn watcher_collects_text_and_stops_at_turn_duration() {
    let tmp = TempDir::new().unwrap();
    let path = write_jsonl(
        tmp.path(),
        "turn.jsonl",
        &[
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello "}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"world"}]}}"#,
            r#"{"type":"system","subtype":"turn_duration","durationMs":500}"#,
        ],
    );

    let result = watch_jsonl_for_turn(&path, 10).await.unwrap();
    assert_eq!(result, "Hello world");
}

#[tokio::test]
async fn watcher_resets_on_compact_boundary() {
    let tmp = TempDir::new().unwrap();
    let path = write_jsonl(
        tmp.path(),
        "compact.jsonl",
        &[
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"OLD"}]}}"#,
            r#"{"type":"system","subtype":"compact_boundary"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"NEW"}]}}"#,
            r#"{"type":"system","subtype":"turn_duration","durationMs":100}"#,
        ],
    );

    let result = watch_jsonl_for_turn(&path, 10).await.unwrap();
    assert_eq!(result, "NEW");
}

#[tokio::test]
async fn watcher_uses_result_text_as_fallback() {
    let tmp = TempDir::new().unwrap();
    let path = write_jsonl(
        tmp.path(),
        "result_fallback.jsonl",
        &[
            r#"{"type":"result","result":"fallback text","session_id":"abc"}"#,
            r#"{"type":"system","subtype":"turn_duration","durationMs":100}"#,
        ],
    );

    let result = watch_jsonl_for_turn(&path, 10).await.unwrap();
    assert_eq!(result, "fallback text");
}

#[tokio::test]
async fn watcher_prefers_assistant_over_result() {
    let tmp = TempDir::new().unwrap();
    let path = write_jsonl(
        tmp.path(),
        "assistant_wins.jsonl",
        &[
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"from assistant"}]}}"#,
            r#"{"type":"result","result":"from result","session_id":"abc"}"#,
            r#"{"type":"system","subtype":"turn_duration","durationMs":100}"#,
        ],
    );

    let result = watch_jsonl_for_turn(&path, 10).await.unwrap();
    assert_eq!(result, "from assistant");
}

#[tokio::test]
async fn watcher_times_out_without_turn_duration() {
    let tmp = TempDir::new().unwrap();
    let path = write_jsonl(
        tmp.path(),
        "no_end.jsonl",
        &[r#"{"type":"assistant","message":{"content":[{"type":"text","text":"x"}]}}"#],
    );

    let err = watch_jsonl_for_turn(&path, 1).await.unwrap_err();
    assert!(
        err.to_string().contains("timed out"),
        "expected timeout error, got: {err}"
    );
}

// ── escape_project_path ──────────────────────────────────────────────────

#[test]
fn escape_project_path_replaces_slashes() {
    let path = Path::new("/home/obj/project/github/RyderFreeman4Logos/cli-sub-agent");
    assert_eq!(
        escape_project_path(path),
        "-home-obj-project-github-RyderFreeman4Logos-cli-sub-agent"
    );
}

#[test]
fn escape_project_path_root() {
    assert_eq!(escape_project_path(Path::new("/")), "-");
}

// ── snapshot and discovery ──────────────────────────────────────────────

#[test]
fn snapshot_captures_only_jsonl_files() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("a.jsonl"), b"").unwrap();
    fs::write(tmp.path().join("b.jsonl"), b"").unwrap();
    fs::write(tmp.path().join("c.json"), b"").unwrap();
    fs::create_dir(tmp.path().join("d.jsonl")).ok(); // directory, not file

    let snap = snapshot_jsonl_files(tmp.path());
    assert_eq!(snap.len(), 2);
    assert!(snap.contains(&tmp.path().join("a.jsonl")));
    assert!(snap.contains(&tmp.path().join("b.jsonl")));
}

#[tokio::test]
async fn discover_new_jsonl_finds_added_file() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("old.jsonl"), b"").unwrap();

    let before = snapshot_jsonl_files(tmp.path());
    assert_eq!(before.len(), 1);

    // Simulate Claude creating a new JSONL after prompt.
    fs::write(tmp.path().join("abc-123-def.jsonl"), b"").unwrap();

    let (path, session_id) = discover_new_jsonl(tmp.path(), &before).await.unwrap();
    assert_eq!(path, tmp.path().join("abc-123-def.jsonl"));
    assert_eq!(session_id, "abc-123-def");
}

#[tokio::test]
async fn discover_new_jsonl_times_out_when_no_new_file() {
    let tmp = TempDir::new().unwrap();
    let before = snapshot_jsonl_files(tmp.path());

    let err = discover_new_jsonl_with_timeout(tmp.path(), &before, Duration::from_secs(2))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("no new JSONL"),
        "expected timeout error, got: {err}"
    );
}

// ── prompt-file delivery ─────────────────────────────────────────────────

#[test]
fn prompt_file_path_uses_session_scoped_temp_file() {
    let path = prompt_file_path_for_session("csa-01ARZ3NDEKTSV4RRFFQ69G5FAV");

    assert_eq!(
        path.file_name().and_then(|name| name.to_str()),
        Some("csa-prompt-csa-01ARZ3NDEKTSV4RRFFQ69G5FAV.md")
    );
    assert!(path.starts_with(std::env::temp_dir()));
}

#[test]
fn prompt_file_instruction_references_file_without_embedding_prompt() {
    let tmp = TempDir::new().unwrap();
    let prompt_file = tmp.path().join("prompt.md");
    let long_prompt = "x".repeat(64 * 1024);

    let instruction = prompt_file_instruction(&prompt_file);

    assert!(instruction.contains(&prompt_file.display().to_string()));
    assert!(!instruction.contains(&long_prompt));
    assert!(
        instruction.len() < 512,
        "file instruction should stay short, got {} bytes",
        instruction.len()
    );
}

#[test]
fn write_prompt_file_persists_full_prompt_once() {
    let tmp = TempDir::new().unwrap();
    let prompt_file = tmp.path().join("prompt.md");
    let prompt = "full prompt\nwith shell chars: $HOME && $(rm -rf nope)\n";

    write_prompt_file(&prompt_file, prompt).unwrap();

    assert_eq!(fs::read_to_string(&prompt_file).unwrap(), prompt);
    let err = write_prompt_file(&prompt_file, "second write").unwrap_err();
    assert!(
        err.to_string().contains("creating tmux prompt file"),
        "expected create_new failure context, got: {err}"
    );
}

#[cfg(unix)]
#[test]
fn write_prompt_file_uses_owner_only_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let prompt_file = tmp.path().join("prompt.md");

    write_prompt_file(&prompt_file, "secret prompt").unwrap();

    let mode = fs::metadata(&prompt_file).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn remove_prompt_file_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let prompt_file = tmp.path().join("prompt.md");
    fs::write(&prompt_file, "prompt").unwrap();

    remove_prompt_file(&prompt_file);
    remove_prompt_file(&prompt_file);

    assert!(!prompt_file.exists());
}

// ── try_read_contract_result ─────────────────────────────────────────────

#[test]
fn read_contract_result_returns_summary_when_present() {
    let tmp = TempDir::new().unwrap();
    let output = tmp.path().join("output");
    fs::create_dir_all(&output).unwrap();
    fs::write(
        output.join("result.toml"),
        "status = \"success\"\nsummary = \"The task is done.\"\nexit_code = 0\n",
    )
    .unwrap();

    let result = try_read_contract_result(tmp.path(), None);
    assert_eq!(result, Some("The task is done.".to_string()));
}

#[test]
fn read_contract_result_reads_nested_result_summary() {
    let tmp = TempDir::new().unwrap();
    let output = tmp.path().join("output");
    fs::create_dir_all(&output).unwrap();
    fs::write(
        output.join("result.toml"),
        "[result]\nstatus = \"success\"\nsummary = \"Nested result.\"\n",
    )
    .unwrap();

    let result = try_read_contract_result(tmp.path(), None);
    assert_eq!(result, Some("Nested result.".to_string()));
}

#[test]
fn read_contract_result_reads_expected_turn_scoped_summary() {
    let tmp = TempDir::new().unwrap();
    let legacy_output = tmp.path().join("output");
    fs::create_dir_all(&legacy_output).unwrap();
    fs::write(
        legacy_output.join("result.toml"),
        "status = \"success\"\nsummary = \"Legacy result.\"\nexit_code = 0\n",
    )
    .unwrap();
    let turn_output = tmp.path().join("output/turns/turn-000002");
    fs::create_dir_all(&turn_output).unwrap();
    fs::write(
        turn_output.join("result.toml"),
        "status = \"success\"\nsummary = \"Latest turn result.\"\nexit_code = 0\n",
    )
    .unwrap();

    let result = try_read_contract_result(
        tmp.path(),
        Some(csa_session::turn_contract_result_artifact_path(2).as_str()),
    );
    assert_eq!(result, Some("Latest turn result.".to_string()));
}

#[test]
fn read_contract_result_does_not_reuse_prior_turn_when_expected_turn_missing() {
    let tmp = TempDir::new().unwrap();
    let prior_turn_output = tmp.path().join("output/turns/turn-000001");
    fs::create_dir_all(&prior_turn_output).unwrap();
    fs::write(
        prior_turn_output.join("result.toml"),
        "status = \"success\"\nsummary = \"Prior turn result.\"\nexit_code = 0\n",
    )
    .unwrap();

    let result = try_read_contract_result(
        tmp.path(),
        Some(csa_session::turn_contract_result_artifact_path(2).as_str()),
    );

    assert_eq!(result, None);
}

#[test]
fn read_contract_result_returns_none_when_missing() {
    let tmp = TempDir::new().unwrap();
    assert!(try_read_contract_result(tmp.path(), None).is_none());
}

#[test]
fn read_contract_result_returns_none_when_no_summary_field() {
    let tmp = TempDir::new().unwrap();
    let output = tmp.path().join("output");
    fs::create_dir_all(&output).unwrap();
    fs::write(output.join("result.toml"), "status = \"success\"\n").unwrap();

    assert!(try_read_contract_result(tmp.path(), None).is_none());
}

// ── create_jsonl_audit_symlink ──────────────────────────────────────────

#[cfg(unix)]
#[test]
fn audit_symlink_links_to_jsonl() {
    let tmp = TempDir::new().unwrap();
    let jsonl = tmp.path().join("conv.jsonl");
    fs::write(&jsonl, b"").unwrap();

    create_jsonl_audit_symlink(tmp.path(), &jsonl);

    let link = tmp.path().join("output").join(JSONL_AUDIT_LINK_NAME);
    assert!(link.exists(), "symlink should exist");
    assert_eq!(fs::read_link(&link).unwrap(), jsonl);
}

#[cfg(unix)]
#[test]
fn audit_symlink_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let jsonl = tmp.path().join("conv.jsonl");
    fs::write(&jsonl, b"").unwrap();

    create_jsonl_audit_symlink(tmp.path(), &jsonl);
    create_jsonl_audit_symlink(tmp.path(), &jsonl);

    let link = tmp.path().join("output").join(JSONL_AUDIT_LINK_NAME);
    assert!(link.exists());
}

// ── list_csa_tmux_sessions ───────────────────────────────────────────────

#[test]
fn list_sessions_returns_empty_when_tmux_unavailable() {
    let result = list_csa_tmux_sessions();
    assert!(
        result.is_ok(),
        "list_csa_tmux_sessions should not error: {result:?}"
    );
}

// ── TmuxTransport capabilities ───────────────────────────────────────────

#[test]
fn capabilities_are_correct() {
    let executor = crate::executor::Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::claude_runtime::claude_runtime_metadata(),
    };
    let transport = TmuxTransport::new(executor);
    let caps = transport.capabilities();
    assert!(!caps.streaming);
    assert!(!caps.session_resume);
    assert!(!caps.session_fork);
    assert!(!caps.typed_events);
    assert_eq!(transport.mode(), TransportMode::Tmux);
}
