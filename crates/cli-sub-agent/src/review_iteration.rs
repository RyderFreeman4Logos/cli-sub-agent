use std::path::Path;

use chrono::{Duration, Utc};
use csa_session::{
    ReviewSessionMeta, find_sessions, get_session_dir, get_session_root, list_sessions,
};

const REVIEW_ITERATION_HEADER: &str = "## Review iteration context";
const MULTI_ROUND_ESCALATION: &str = "Multiple prior rounds have fired on this branch. Oscillating verdicts across rounds indicate design residuals, not bugs. Strongly prefer PASS for any finding that overlaps with prior rounds' concerns — FAIL only for NEW correctness bugs (crash, data loss, contract violation) not previously raised.";

pub(crate) fn count_prior_reviews_for_branch(project_root: &Path, branch: &str) -> usize {
    let current_session_id = std::env::var("CSA_SESSION_ID").ok();
    let sessions = match find_sessions(project_root, Some(branch), None, None, None) {
        Ok(sessions) => sessions,
        Err(_) => return 0,
    };

    if sessions.is_empty() {
        let all_sessions = match list_sessions(project_root, None) {
            Ok(sessions) => sessions,
            Err(_) => return 0,
        };
        let saw_branch_metadata = all_sessions
            .iter()
            .any(|session| session.resolved_identity().ref_name.is_some());
        if !saw_branch_metadata {
            // Older session state may not persist branch/ref metadata reliably.
            // Fall back to counting recent review sessions so repeated same-day
            // review loops still receive anti-flip guidance instead of silently
            // dropping all iteration context.
            return count_recent_reviews(project_root, current_session_id.as_deref());
        }
        return count_recent_reviews(project_root, current_session_id.as_deref());
    }

    sessions
        .into_iter()
        .filter(|session| current_session_id.as_deref() != Some(session.meta_session_id.as_str()))
        .filter_map(|session| load_review_meta(project_root, &session.meta_session_id))
        .count()
}

pub(crate) fn render_review_iteration_context(project_root: &Path, branch: &str) -> Option<String> {
    let prior_count = count_prior_reviews_for_branch(project_root, branch);
    if prior_count == 0 {
        return None;
    }

    let mut rendered = format!(
        "{REVIEW_ITERATION_HEADER}\n\nThis is review iteration {} on branch '{branch}'. Prior review count on this branch: {prior_count}.\n",
        prior_count + 1
    );
    if prior_count >= 3 {
        rendered.push('\n');
        rendered.push_str(MULTI_ROUND_ESCALATION);
        rendered.push('\n');
    }
    Some(rendered)
}

fn count_recent_reviews(project_root: &Path, exclude_session_id: Option<&str>) -> usize {
    let cutoff = Utc::now() - Duration::hours(24);
    let session_dirs = match list_session_dirs(project_root) {
        Ok(session_dirs) => session_dirs,
        Err(_) => return 0,
    };

    session_dirs
        .into_iter()
        .filter(|session_dir| {
            exclude_session_id != session_dir.file_name().and_then(|name| name.to_str())
        })
        .filter_map(|session_dir| load_review_meta_from_dir(&session_dir))
        .filter(|meta| meta.timestamp >= cutoff)
        .count()
}

fn list_session_dirs(project_root: &Path) -> std::io::Result<Vec<std::path::PathBuf>> {
    let session_root = get_session_root(project_root)
        .map_err(std::io::Error::other)?
        .join("sessions");
    if !session_root.is_dir() {
        return Ok(Vec::new());
    }

    let mut session_dirs = Vec::new();
    for entry in std::fs::read_dir(session_root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            session_dirs.push(entry.path());
        }
    }
    Ok(session_dirs)
}

fn load_review_meta(project_root: &Path, session_id: &str) -> Option<ReviewSessionMeta> {
    let session_dir = get_session_dir(project_root, session_id).ok()?;
    load_review_meta_from_dir(&session_dir)
}

fn load_review_meta_from_dir(session_dir: &Path) -> Option<ReviewSessionMeta> {
    let review_meta_path = session_dir.join("review_meta.json");
    if !review_meta_path.is_file() {
        return None;
    }

    let content = std::fs::read_to_string(review_meta_path).ok()?;
    serde_json::from_str(&content).ok()
}
