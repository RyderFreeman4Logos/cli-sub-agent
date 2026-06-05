use crate::state::MetaSessionState;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use super::{STATE_FILE_NAME, save_session_in};

pub(super) fn recover_corrupt_session_state(
    base_dir: &Path,
    session_dir: &Path,
    session_id: &str,
    error: &anyhow::Error,
) -> Option<MetaSessionState> {
    // BUG-11: Corrupt state.toml recovery
    let state_path = session_dir.join(STATE_FILE_NAME);
    if !state_path.exists() {
        tracing::warn!(session_id = %session_id, "No state.toml");
        return None;
    }
    let backup_path = session_dir.join("state.toml.corrupt");
    if let Err(backup_err) = fs::rename(&state_path, &backup_path) {
        tracing::warn!(
            session_id = %session_id,
            error = %backup_err,
            "Failed to backup corrupt state.toml"
        );
        return None;
    }
    tracing::warn!(
        session_id = %session_id,
        error = %error,
        "Recovered corrupt state.toml → state.toml.corrupt"
    );
    let now = chrono::Utc::now();
    let minimal_state = MetaSessionState {
        meta_session_id: session_id.to_string(),
        description: Some("(recovered from corrupt state)".to_string()),
        project_path: "(unknown)".to_string(),
        branch: None,
        created_at: now,
        last_accessed: now,
        csa_version: None,
        genealogy: crate::state::Genealogy::default(),
        tools: HashMap::new(),
        context_status: Default::default(),
        total_token_usage: None,
        phase: Default::default(),
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
    if let Err(save_err) = save_session_in(base_dir, &minimal_state) {
        tracing::warn!(
            session_id = %session_id,
            error = %save_err,
            "Failed to save minimal state after recovery"
        );
        return None;
    }
    Some(minimal_state)
}
