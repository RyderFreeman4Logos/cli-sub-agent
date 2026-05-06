use super::*;
use crate::test_env::TEST_ENV_LOCK;
use std::process::Output;
use std::time::Duration;
use tempfile::tempdir;

const ETXTBSY_RAW_OS_ERROR: i32 = 26;

fn ensure_git_init_with_etxtbsy_retry(sessions_dir: &Path) {
    for attempt in 0..=3 {
        match crate::git::ensure_git_init(sessions_dir) {
            Ok(()) => return,
            Err(err) if is_text_file_busy(&err) && attempt < 3 => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(err) => panic!("failed to initialize test git repository: {err:#}"),
        }
    }
}

fn is_text_file_busy(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::raw_os_error)
            == Some(ETXTBSY_RAW_OS_ERROR)
    })
}

fn run_git_output_with_etxtbsy_retry(sessions_dir: &Path, args: &[&str]) -> Output {
    let max_attempts = 4;
    for attempt in 0..max_attempts {
        match Command::new("git")
            .args(args)
            .current_dir(sessions_dir)
            .output()
        {
            Ok(output) => return output,
            Err(err)
                if err.raw_os_error() == Some(ETXTBSY_RAW_OS_ERROR)
                    && attempt + 1 < max_attempts =>
            {
                std::thread::sleep(Duration::from_millis(25_u64 << attempt));
            }
            Err(err) => panic!("git {args:?} failed: {err}"),
        }
    }
    unreachable!("retry loop should have returned or panicked")
}

fn stage_session_dir_for_commit(sessions_dir: &Path, session_id: &str) {
    let session_path = format!("{session_id}/");
    let output = run_git_output_with_etxtbsy_retry(sessions_dir, &["add", "--", &session_path]);
    assert!(
        output.status.success(),
        "git add failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn has_staged_session_changes(sessions_dir: &Path, session_id: &str) -> bool {
    let session_path = format!("{session_id}/");
    let output = run_git_output_with_etxtbsy_retry(
        sessions_dir,
        &["diff", "--cached", "--quiet", "--", &session_path],
    );

    match output.status.code() {
        Some(0) => false,
        Some(1) => true,
        Some(code) => panic!(
            "git diff --cached failed (exit {code}): {}",
            String::from_utf8_lossy(&output.stderr)
        ),
        None => panic!("git diff --cached terminated by signal"),
    }
}

fn make_note() -> CheckpointNote {
    CheckpointNote {
        session_id: "01TESTID000000000000000000".to_string(),
        tool: Some("codex".to_string()),
        status: "Completed".to_string(),
        created_at: "2026-02-13T10:00:00+00:00".to_string(),
        completed_at: "2026-02-13T10:05:00+00:00".to_string(),
        turn_count: 3,
        token_usage: Some(TokenUsageSummary {
            input_tokens: 5000,
            output_tokens: 1200,
        }),
        description: Some("Test session".to_string()),
        op_id: None,
    }
}

#[test]
fn test_checkpoint_note_toml_roundtrip() {
    let note = make_note();
    let toml_str = toml::to_string_pretty(&note).unwrap();
    let parsed: CheckpointNote = toml::from_str(&toml_str).unwrap();
    assert_eq!(note, parsed);
}

#[test]
fn test_checkpoint_note_toml_format() {
    let note = make_note();
    let toml_str = toml::to_string_pretty(&note).unwrap();
    assert!(toml_str.contains("session_id = \"01TESTID000000000000000000\""));
    assert!(toml_str.contains("tool = \"codex\""));
    assert!(toml_str.contains("turn_count = 3"));
    assert!(toml_str.contains("input_tokens = 5000"));
}

#[test]
fn test_checkpoint_note_without_optional_fields() {
    let note = CheckpointNote {
        session_id: "01TESTID000000000000000000".to_string(),
        tool: None,
        status: "Running".to_string(),
        created_at: "2026-02-13T10:00:00+00:00".to_string(),
        completed_at: "2026-02-13T10:00:00+00:00".to_string(),
        turn_count: 0,
        token_usage: None,
        description: None,
        op_id: None,
    };
    let toml_str = toml::to_string_pretty(&note).unwrap();
    let parsed: CheckpointNote = toml::from_str(&toml_str).unwrap();
    assert_eq!(note, parsed);
}

#[test]
fn test_emit_and_read_session_checkpoints_roundtrip() {
    let tmp = tempdir().unwrap();
    let session_dir = tmp.path().join("01TESTSESSION");
    std::fs::create_dir_all(&session_dir).unwrap();

    let first = emit_checkpoint(&session_dir, "plan", "Started planning").unwrap();
    let second = emit_checkpoint(&session_dir, "run", "Running worker").unwrap();
    let third = emit_checkpoint(&session_dir, "review", "Reviewing result").unwrap();

    assert_eq!(
        first.file_name().and_then(|n| n.to_str()),
        Some("0001.toml")
    );
    assert_eq!(
        second.file_name().and_then(|n| n.to_str()),
        Some("0002.toml")
    );
    assert_eq!(
        third.file_name().and_then(|n| n.to_str()),
        Some("0003.toml")
    );

    let latest = read_latest_checkpoint(&session_dir).unwrap().unwrap();
    assert_eq!(latest.sequence, 3);
    assert_eq!(latest.phase, "review");
    assert_eq!(latest.summary, "Reviewing result");

    let checkpoints = read_checkpoints(&session_dir).unwrap();
    assert_eq!(checkpoints.len(), 3);
    assert_eq!(
        checkpoints
            .iter()
            .map(|checkpoint| checkpoint.sequence)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(
        checkpoints
            .iter()
            .map(|checkpoint| checkpoint.phase.as_str())
            .collect::<Vec<_>>(),
        vec!["plan", "run", "review"]
    );
}

#[test]
fn test_write_checkpoint_no_commits_errors() {
    let _env_lock = TEST_ENV_LOCK.lock().expect("env lock poisoned");
    let tmp = tempdir().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    crate::git::ensure_git_init(&sessions_dir).unwrap();

    let note = make_note();
    let result = write_checkpoint_note(&sessions_dir, &note);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("commit") || err_msg.contains("No commits"),
        "Should mention commits, got: {err_msg}"
    );
}

#[test]
fn test_checkpoint_targets_session_commit_not_head() {
    let _env_lock = TEST_ENV_LOCK.lock().expect("env lock poisoned");
    let tmp = tempdir().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    crate::git::ensure_git_init(&sessions_dir).unwrap();

    let session_a = ulid::Ulid::new().to_string();
    let dir_a = sessions_dir.join(&session_a);
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::write(dir_a.join("state.toml"), "a = true").unwrap();
    crate::git::commit_session(&sessions_dir, &session_a, "session A").unwrap();

    let commit_a = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&sessions_dir)
        .output()
        .unwrap();
    let commit_a_sha = String::from_utf8_lossy(&commit_a.stdout).trim().to_string();

    let session_b = ulid::Ulid::new().to_string();
    let dir_b = sessions_dir.join(&session_b);
    std::fs::create_dir_all(&dir_b).unwrap();
    std::fs::write(dir_b.join("state.toml"), "b = true").unwrap();
    crate::git::commit_session(&sessions_dir, &session_b, "session B").unwrap();

    let mut note = make_note();
    note.session_id.clone_from(&session_a);
    write_checkpoint_note(&sessions_dir, &note).unwrap();

    let read_back = read_checkpoint_note(&sessions_dir, &commit_a_sha).unwrap();
    assert!(read_back.is_some(), "Note should be on session A's commit");
    assert_eq!(read_back.unwrap().session_id, session_a);
}

#[test]
fn test_note_from_session_deterministic_tool_selection() {
    let session = crate::MetaSessionState {
        meta_session_id: "01TEST".to_string(),
        description: None,
        project_path: "/tmp".to_string(),
        branch: None,
        created_at: chrono::Utc::now(),
        last_accessed: chrono::Utc::now(),
        csa_version: None,
        genealogy: Default::default(),
        tools: {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "gemini-cli".to_string(),
                crate::state::ToolState {
                    provider_session_id: None,
                    last_action_summary: String::new(),
                    last_exit_code: 0,
                    tool_version: None,
                    token_usage: None,
                    updated_at: chrono::Utc::now(),
                },
            );
            m.insert(
                "codex".to_string(),
                crate::state::ToolState {
                    provider_session_id: None,
                    last_action_summary: String::new(),
                    last_exit_code: 0,
                    tool_version: None,
                    token_usage: None,
                    updated_at: chrono::Utc::now(),
                },
            );
            m
        },
        context_status: Default::default(),
        total_token_usage: None,
        phase: crate::state::SessionPhase::Active,
        task_context: Default::default(),
        turn_count: 0,
        token_budget: None,
        sandbox_info: None,
        termination_reason: None,
        is_seed_candidate: false,
        git_head_at_creation: None,
        pre_session_porcelain: None,
        last_return_packet: None,
        change_id: None,
        spec_id: None,
        fork_call_timestamps: Vec::new(),
        vcs_identity: None,
        identity_version: 1,
    };

    let note = note_from_session(&session);
    assert_eq!(note.tool, Some("codex".to_string()));
}

#[test]
fn test_write_and_read_checkpoint_roundtrip() {
    let _env_lock = TEST_ENV_LOCK.lock().expect("env lock poisoned");
    let tmp = tempdir().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    ensure_git_init_with_etxtbsy_retry(&sessions_dir);

    let session_id = ulid::Ulid::new().to_string();
    let session_dir = sessions_dir.join(&session_id);
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(session_dir.join("state.toml"), "test = true").unwrap();
    stage_session_dir_for_commit(&sessions_dir, &session_id);
    if !has_staged_session_changes(&sessions_dir, &session_id) {
        std::fs::write(
            session_dir.join("state.toml"),
            "test = true\nroundtrip = true",
        )
        .unwrap();
        stage_session_dir_for_commit(&sessions_dir, &session_id);
    }
    assert!(
        has_staged_session_changes(&sessions_dir, &session_id),
        "test fixture must have staged changes before commit_session"
    );
    crate::git::commit_session(&sessions_dir, &session_id, "test session").unwrap();

    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&sessions_dir)
        .output()
        .unwrap();
    let head_sha = String::from_utf8_lossy(&head.stdout).trim().to_string();

    let mut note = make_note();
    note.session_id.clone_from(&session_id);
    write_checkpoint_note(&sessions_dir, &note).unwrap();

    let read_back = read_checkpoint_note(&sessions_dir, &head_sha).unwrap();
    assert!(read_back.is_some());
    assert_eq!(read_back.unwrap(), note);
}

#[test]
fn test_read_checkpoint_no_note_returns_none() {
    let _env_lock = TEST_ENV_LOCK.lock().expect("env lock poisoned");
    let tmp = tempdir().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    crate::git::ensure_git_init(&sessions_dir).unwrap();

    let session_id = ulid::Ulid::new().to_string();
    let session_dir = sessions_dir.join(&session_id);
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(session_dir.join("state.toml"), "test = true").unwrap();
    crate::git::commit_session(&sessions_dir, &session_id, "test").unwrap();

    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&sessions_dir)
        .output()
        .unwrap();
    let head_sha = String::from_utf8_lossy(&head.stdout).trim().to_string();

    let result = read_checkpoint_note(&sessions_dir, &head_sha).unwrap();
    assert!(result.is_none());
}

#[test]
fn test_list_checkpoints_empty_repo() {
    let _env_lock = TEST_ENV_LOCK.lock().expect("env lock poisoned");
    let tmp = tempdir().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    crate::git::ensure_git_init(&sessions_dir).unwrap();

    let results = list_checkpoint_notes(&sessions_dir).unwrap();
    assert!(results.is_empty());
}

#[test]
fn test_list_checkpoints_with_notes() {
    let _env_lock = TEST_ENV_LOCK.lock().expect("env lock poisoned");
    let tmp = tempdir().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    ensure_git_init_with_etxtbsy_retry(&sessions_dir);

    let session_id = ulid::Ulid::new().to_string();
    let session_dir = sessions_dir.join(&session_id);
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(session_dir.join("state.toml"), "test = true").unwrap();
    crate::git::commit_session(&sessions_dir, &session_id, "test").unwrap();

    let mut note = make_note();
    note.session_id.clone_from(&session_id);
    write_checkpoint_note(&sessions_dir, &note).unwrap();

    let results = list_checkpoint_notes(&sessions_dir).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].1.session_id, session_id);
}

#[test]
fn test_note_from_session() {
    let session = crate::MetaSessionState {
        meta_session_id: "01TEST".to_string(),
        description: Some("test desc".to_string()),
        project_path: "/tmp".to_string(),
        branch: None,
        created_at: chrono::Utc::now(),
        last_accessed: chrono::Utc::now(),
        csa_version: None,
        genealogy: Default::default(),
        tools: {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "codex".to_string(),
                crate::state::ToolState {
                    provider_session_id: None,
                    last_action_summary: String::new(),
                    last_exit_code: 0,
                    tool_version: None,
                    token_usage: None,
                    updated_at: chrono::Utc::now(),
                },
            );
            m
        },
        context_status: Default::default(),
        total_token_usage: Some(crate::state::TokenUsage {
            input_tokens: Some(1000),
            output_tokens: Some(500),
            total_tokens: Some(1500),
            estimated_cost_usd: None,
        }),
        phase: crate::state::SessionPhase::Retired,
        task_context: Default::default(),
        turn_count: 5,
        token_budget: None,
        sandbox_info: None,
        termination_reason: None,
        is_seed_candidate: false,
        git_head_at_creation: None,
        pre_session_porcelain: None,
        last_return_packet: None,
        change_id: None,
        spec_id: None,
        fork_call_timestamps: Vec::new(),
        vcs_identity: None,
        identity_version: 1,
    };

    let note = note_from_session(&session);
    assert_eq!(note.session_id, "01TEST");
    assert_eq!(note.tool, Some("codex".to_string()));
    assert_eq!(note.status, "Retired");
    assert_eq!(note.turn_count, 5);
    assert!(note.token_usage.is_some());
    let usage = note.token_usage.unwrap();
    assert_eq!(usage.input_tokens, 1000);
    assert_eq!(usage.output_tokens, 500);
    assert_eq!(note.description, Some("test desc".to_string()));
}

#[test]
fn test_list_checkpoints_skips_malformed_note_with_warning() {
    let _env_lock = TEST_ENV_LOCK.lock().expect("env lock poisoned");
    let tmp = tempdir().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    crate::git::ensure_git_init(&sessions_dir).unwrap();

    let session_id = ulid::Ulid::new().to_string();
    let session_dir = sessions_dir.join(&session_id);
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(session_dir.join("state.toml"), "test = true").unwrap();
    crate::git::commit_session(&sessions_dir, &session_id, "test").unwrap();

    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&sessions_dir)
        .output()
        .unwrap();
    let head_sha = String::from_utf8_lossy(&head.stdout).trim().to_string();

    let output = Command::new("git")
        .args([
            "notes",
            "--ref=refs/notes/csa-checkpoints",
            "add",
            "-f",
            "-m",
            "this is not valid TOML {{{",
            &head_sha,
        ])
        .current_dir(&sessions_dir)
        .output()
        .unwrap();
    assert!(output.status.success());

    let results = list_checkpoint_notes(&sessions_dir).unwrap();
    assert!(results.is_empty(), "Malformed note should be skipped");
}

#[test]
fn test_write_checkpoint_mismatched_session_id_targets_correct_commit() {
    let _env_lock = TEST_ENV_LOCK.lock().expect("env lock poisoned");
    let tmp = tempdir().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    crate::git::ensure_git_init(&sessions_dir).unwrap();

    let session_a = ulid::Ulid::new().to_string();
    let dir_a = sessions_dir.join(&session_a);
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::write(dir_a.join("state.toml"), "a = true").unwrap();
    crate::git::commit_session(&sessions_dir, &session_a, "session A").unwrap();

    let mut note = make_note();
    note.session_id = "NONEXISTENT_SESSION_ID_00000".to_string();

    let result = write_checkpoint_note(&sessions_dir, &note);
    assert!(
        result.is_err(),
        "Should fail when session_id has no matching commits"
    );
}
