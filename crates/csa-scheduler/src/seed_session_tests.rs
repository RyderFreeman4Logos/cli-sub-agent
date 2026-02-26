use super::*;
use csa_session::state::{
    ContextStatus, Genealogy, MetaSessionState, SessionPhase, TaskContext, ToolState,
};
use std::collections::HashMap;

fn make_session(
    id: &str,
    tool: &str,
    phase: SessionPhase,
    is_seed: bool,
    age_hours: i64,
    git_head: Option<&str>,
    is_fork: bool,
) -> MetaSessionState {
    let now = chrono::Utc::now();
    let accessed = now - chrono::Duration::hours(age_hours);

    let mut tools = HashMap::new();
    tools.insert(
        tool.to_string(),
        ToolState {
            provider_session_id: Some(format!("provider-{id}")),
            last_action_summary: "test".to_string(),
            last_exit_code: 0,
            updated_at: accessed,
            token_usage: None,
        },
    );

    MetaSessionState {
        meta_session_id: id.to_string(),
        description: Some(format!("session {id}")),
        project_path: "/tmp/test".to_string(),
        branch: None,
        created_at: accessed,
        last_accessed: accessed,
        genealogy: Genealogy {
            fork_of_session_id: if is_fork {
                Some("01PARENT".to_string())
            } else {
                None
            },
            ..Default::default()
        },
        tools,
        context_status: ContextStatus::default(),
        total_token_usage: None,
        phase,
        task_context: TaskContext::default(),
        turn_count: 1,
        token_budget: None,
        sandbox_info: None,
        termination_reason: None,
        is_seed_candidate: is_seed,
        git_head_at_creation: git_head.map(|s| s.to_string()),
        last_return_packet: None,
        fork_call_timestamps: Vec::new(),
    }
}

// ── is_seed_valid tests ────────────────────────────────────────────

#[test]
fn test_valid_seed() {
    let session = make_session(
        "01A",
        "codex",
        SessionPhase::Available,
        true,
        1,
        Some("abc123"),
        false,
    );
    assert!(is_seed_valid(&session, 86400, Some("abc123")));
}

#[test]
fn test_seed_invalid_not_candidate() {
    let session = make_session(
        "01B",
        "codex",
        SessionPhase::Available,
        false,
        1,
        Some("abc123"),
        false,
    );
    assert!(!is_seed_valid(&session, 86400, Some("abc123")));
}

#[test]
fn test_seed_invalid_wrong_phase() {
    let session = make_session(
        "01C",
        "codex",
        SessionPhase::Active,
        true,
        1,
        Some("abc123"),
        false,
    );
    assert!(!is_seed_valid(&session, 86400, Some("abc123")));
}

#[test]
fn test_seed_invalid_too_old() {
    let session = make_session(
        "01D",
        "codex",
        SessionPhase::Available,
        true,
        48,
        Some("abc123"),
        false,
    );
    // max age 24h = 86400s, session is 48h old
    assert!(!is_seed_valid(&session, 86400, Some("abc123")));
}

#[test]
fn test_seed_invalid_git_head_mismatch() {
    let session = make_session(
        "01E",
        "codex",
        SessionPhase::Available,
        true,
        1,
        Some("abc123"),
        false,
    );
    assert!(!is_seed_valid(&session, 86400, Some("def456")));
}

#[test]
fn test_seed_valid_no_git_head_on_session() {
    // If session lacks git HEAD, skip the check
    let session = make_session(
        "01F",
        "codex",
        SessionPhase::Available,
        true,
        1,
        None,
        false,
    );
    assert!(is_seed_valid(&session, 86400, Some("abc123")));
}

#[test]
fn test_seed_valid_no_current_git_head() {
    // If we don't know current HEAD, skip the check
    let session = make_session(
        "01G",
        "codex",
        SessionPhase::Available,
        true,
        1,
        Some("abc123"),
        false,
    );
    assert!(is_seed_valid(&session, 86400, None));
}

#[test]
fn test_seed_valid_both_no_git_head() {
    let session = make_session(
        "01H",
        "codex",
        SessionPhase::Available,
        true,
        1,
        None,
        false,
    );
    assert!(is_seed_valid(&session, 86400, None));
}

#[test]
fn test_seed_valid_at_exact_max_age() {
    // Session exactly at the boundary (24h = 86400s)
    let session = make_session(
        "01I",
        "codex",
        SessionPhase::Available,
        true,
        24,
        Some("abc"),
        false,
    );
    // 24 hours in seconds = 86400, should be valid (<=)
    assert!(is_seed_valid(&session, 86400, Some("abc")));
}

#[test]
fn test_seed_invalid_retired_phase() {
    let session = make_session(
        "01J",
        "codex",
        SessionPhase::Retired,
        true,
        1,
        Some("abc"),
        false,
    );
    assert!(!is_seed_valid(&session, 86400, Some("abc")));
}
