use super::*;
use chrono::{TimeZone, Utc};
use csa_core::vcs::{VcsIdentity, VcsKind};
use csa_session::{Genealogy, MetaSessionState, SessionPhase, TaskContext};
use std::collections::HashMap;

fn test_session(
    meta_session_id: &str,
    last_accessed: chrono::DateTime<Utc>,
    phase: SessionPhase,
) -> MetaSessionState {
    MetaSessionState {
        meta_session_id: meta_session_id.to_string(),
        description: None,
        project_path: "/tmp/project".to_string(),
        branch: None,
        created_at: last_accessed,
        last_accessed,
        genealogy: Genealogy {
            parent_session_id: None,
            depth: 0,
            ..Default::default()
        },
        tools: HashMap::new(),
        context_status: Default::default(),
        total_token_usage: None,
        phase,
        task_context: TaskContext::default(),
        turn_count: 0,
        token_budget: None,
        sandbox_info: None,
        termination_reason: None,
        is_seed_candidate: false,
        git_head_at_creation: None,
        last_return_packet: None,
        change_id: None,
        spec_id: None,
        fork_call_timestamps: Vec::new(),
        vcs_identity: None,
        identity_version: 1,
    }
}

#[test]
fn interrupted_skill_session_matches_current_vcs_requires_same_branch_and_head() {
    let mut session = test_session(
        "01KJTESTSIGTERMABCDE12345",
        Utc.with_ymd_and_hms(2026, 3, 1, 13, 10, 0)
            .single()
            .unwrap(),
        SessionPhase::Active,
    );
    session.vcs_identity = Some(VcsIdentity {
        vcs_kind: VcsKind::Git,
        commit_id: Some("abc123".to_string()),
        ref_name: Some("fix/open-issues-20260408".to_string()),
        ..Default::default()
    });
    session.identity_version = 2;

    let matching = VcsIdentity {
        vcs_kind: VcsKind::Git,
        commit_id: Some("abc123".to_string()),
        ref_name: Some("fix/open-issues-20260408".to_string()),
        ..Default::default()
    };
    assert!(interrupted_skill_session_matches_current_vcs(
        &session, &matching
    ));

    let different_head = VcsIdentity {
        vcs_kind: VcsKind::Git,
        commit_id: Some("def456".to_string()),
        ref_name: Some("fix/open-issues-20260408".to_string()),
        ..Default::default()
    };
    assert!(!interrupted_skill_session_matches_current_vcs(
        &session,
        &different_head
    ));

    let different_branch = VcsIdentity {
        vcs_kind: VcsKind::Git,
        commit_id: Some("abc123".to_string()),
        ref_name: Some("main".to_string()),
        ..Default::default()
    };
    assert!(!interrupted_skill_session_matches_current_vcs(
        &session,
        &different_branch
    ));
}
