//! Seed session management: discovery, validation, and LRU eviction.
//!
//! Seed sessions are warm, compressed sessions that can be soft-forked
//! to avoid cold starts. A session becomes a seed candidate after
//! successful completion, and is invalidated by age or git HEAD drift.

use anyhow::Result;
use chrono::Utc;
use csa_session::MetaSessionState;
use std::path::Path;
use tracing::{debug, info};

/// Result of a seed session lookup.
#[derive(Debug, Clone)]
pub struct SeedCandidate {
    /// CSA session ID of the seed.
    pub session_id: String,
    /// Tool name the seed was created with.
    pub tool_name: String,
}

/// Find the best seed session for a tool×project combination.
///
/// Returns the most recent session that:
/// - Is in `Available` phase (compressed, not retired)
/// - Is marked as `is_seed_candidate`
/// - Is NOT a fork child itself (no `fork_of_session_id`)
/// - Is younger than `seed_max_age_secs`
/// - Has a matching git HEAD (if tracked)
/// - Matches the requested tool AND project
/// - Has a non-empty `provider_session_id` when `require_provider_session` is true
///   (native forks need the provider session ID to resume)
pub fn find_seed_session(
    project_root: &Path,
    tool: &str,
    seed_max_age_secs: u64,
    current_git_head: Option<&str>,
) -> Result<Option<SeedCandidate>> {
    find_seed_session_inner(
        project_root,
        tool,
        seed_max_age_secs,
        current_git_head,
        false,
    )
}

/// Like [`find_seed_session`] but with explicit control over provider session filtering.
///
/// When `require_provider_session` is true, only candidates whose tool state has a
/// non-empty `provider_session_id` are returned. Use this for tools that require
/// native fork (e.g. claude-code) where a missing provider session would cause the
/// fork to fail at execution time.
pub fn find_seed_session_for_native_fork(
    project_root: &Path,
    tool: &str,
    seed_max_age_secs: u64,
    current_git_head: Option<&str>,
) -> Result<Option<SeedCandidate>> {
    find_seed_session_inner(
        project_root,
        tool,
        seed_max_age_secs,
        current_git_head,
        true,
    )
}

fn find_seed_session_inner(
    project_root: &Path,
    tool: &str,
    seed_max_age_secs: u64,
    current_git_head: Option<&str>,
    require_provider_session: bool,
) -> Result<Option<SeedCandidate>> {
    let sessions = csa_session::list_sessions(project_root, None)?;
    let now = Utc::now();

    let mut candidates: Vec<&MetaSessionState> = sessions
        .iter()
        .filter(|s| {
            // Must be Available phase
            s.phase == csa_session::SessionPhase::Available
            // Must be a seed candidate
            && s.is_seed_candidate
            // Must NOT be a fork child
            && !s.genealogy.is_fork()
            // Must have the requested tool
            && s.tools.contains_key(tool)
            // Native fork readiness: provider_session_id must be present
            && (!require_provider_session || s.tools.get(tool).is_some_and(|ts| ts.provider_session_id.is_some()))
            // Age check
            && {
                let age_secs = (now - s.last_accessed).num_seconds().max(0) as u64;
                age_secs <= seed_max_age_secs
            }
            // Git HEAD check: if both sides have a HEAD, they must match
            && match (current_git_head, s.git_head_at_creation.as_deref()) {
                (Some(current), Some(stored)) => current == stored,
                // If either side lacks HEAD info, skip this check
                _ => true,
            }
        })
        .collect();

    // Sort by last_accessed descending (most recent first)
    candidates.sort_by(|a, b| b.last_accessed.cmp(&a.last_accessed));

    if let Some(best) = candidates.first() {
        debug!(
            session_id = %best.meta_session_id,
            tool = %tool,
            age_secs = (now - best.last_accessed).num_seconds(),
            "Found seed session candidate"
        );
        Ok(Some(SeedCandidate {
            session_id: best.meta_session_id.clone(),
            tool_name: tool.to_string(),
        }))
    } else {
        debug!(tool = %tool, "No seed session found");
        Ok(None)
    }
}

/// Enforce the `max_seed_sessions` limit per tool×project via LRU eviction.
///
/// Finds all seed candidates for the given tool, sorts by `last_accessed`
/// descending, and retires any beyond the limit.
pub fn evict_excess_seeds(
    project_root: &Path,
    tool: &str,
    max_seed_sessions: u32,
) -> Result<Vec<String>> {
    let sessions = csa_session::list_sessions(project_root, None)?;

    let mut seeds: Vec<MetaSessionState> = sessions
        .into_iter()
        .filter(|s| {
            s.phase == csa_session::SessionPhase::Available
                && s.is_seed_candidate
                && s.tools.contains_key(tool)
        })
        .collect();

    // Sort by last_accessed descending (keep most recent)
    seeds.sort_by(|a, b| b.last_accessed.cmp(&a.last_accessed));

    let mut retired_ids = Vec::new();

    // Retire seeds beyond the limit
    for seed in seeds.iter().skip(max_seed_sessions as usize) {
        let mut state =
            csa_session::load_session(Path::new(&seed.project_path), &seed.meta_session_id)?;
        match state.phase.transition(&csa_session::PhaseEvent::Retired) {
            Ok(new_phase) => {
                state.phase = new_phase;
                state.is_seed_candidate = false;
                csa_session::save_session(&state)?;
                info!(
                    session_id = %state.meta_session_id,
                    tool = %tool,
                    "Retired excess seed session (LRU eviction)"
                );
                retired_ids.push(state.meta_session_id.clone());
            }
            Err(e) => {
                debug!(
                    session_id = %state.meta_session_id,
                    error = %e,
                    "Could not retire seed session"
                );
            }
        }
    }

    Ok(retired_ids)
}

/// Check whether a session is a valid seed (not expired, git HEAD matches).
///
/// This is a pure validation function that does not load sessions from disk.
pub fn is_seed_valid(
    session: &MetaSessionState,
    seed_max_age_secs: u64,
    current_git_head: Option<&str>,
) -> bool {
    if !session.is_seed_candidate {
        return false;
    }
    if session.phase != csa_session::SessionPhase::Available {
        return false;
    }

    // Age check
    let now = Utc::now();
    let age_secs = (now - session.last_accessed).num_seconds().max(0) as u64;
    if age_secs > seed_max_age_secs {
        return false;
    }

    // Git HEAD check
    match (current_git_head, session.git_head_at_creation.as_deref()) {
        (Some(current), Some(stored)) => current == stored,
        _ => true,
    }
}

#[cfg(test)]
#[path = "seed_session_tests.rs"]
mod tests;
