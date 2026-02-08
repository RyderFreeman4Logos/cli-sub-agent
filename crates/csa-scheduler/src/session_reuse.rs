//! Session intelligent reuse: find compacted sessions eligible for new tasks.

use anyhow::Result;
use csa_session::MetaSessionState;
use serde::Serialize;
use std::path::Path;
use tracing::debug;

/// A session that can be reused for a new task.
#[derive(Debug, Clone, Serialize)]
pub struct ReuseCandidate {
    pub session_id: String,
    pub tool_name: String,
    pub score: f64,
    pub reason: String,
}

/// Find sessions that are available for reuse.
///
/// Conditions for reuse:
/// - `phase == Available` (session has been compacted and is waiting)
/// - Session has a ToolState for at least one of the `tier_tools`
/// - Optional: `task_context.task_type` is related to `task_type`
///
/// Returns candidates sorted by relevance score (highest first).
pub fn find_reusable_sessions(
    project_root: &Path,
    task_type: &str,
    tier_tools: &[String],
) -> Result<Vec<ReuseCandidate>> {
    let sessions = csa_session::list_sessions(project_root, None)?;
    let mut candidates = Vec::new();

    for session in sessions {
        // Only consider Available sessions
        if session.phase != csa_session::state::SessionPhase::Available {
            continue;
        }

        // Check if session has a tool state for any of the tier tools
        for tool in tier_tools {
            if session.tools.contains_key(tool) {
                let score = compute_relevance_score(&session, task_type, tool);
                if score > 0.0 {
                    candidates.push(ReuseCandidate {
                        session_id: session.meta_session_id.clone(),
                        tool_name: tool.clone(),
                        score,
                        reason: format!(
                            "phase=Available, tool={}, task_match={}",
                            tool,
                            session.task_context.task_type.as_deref().unwrap_or("none"),
                        ),
                    });
                }
            }
        }
    }

    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    debug!(
        count = candidates.len(),
        task_type = %task_type,
        "Found reusable session candidates"
    );

    Ok(candidates)
}

/// Compute relevance score for a session-tool pair.
fn compute_relevance_score(session: &MetaSessionState, task_type: &str, _tool: &str) -> f64 {
    let mut score = 0.3; // Base score for being Available + having the tool

    // Exact task_type match
    if let Some(ref st) = session.task_context.task_type {
        if st == task_type {
            score += 1.0;
        } else if is_related_task(st, task_type) {
            score += 0.5;
        }
    }

    // Recency bonus: sessions compacted more recently get a small bonus
    let age_hours = (chrono::Utc::now() - session.last_accessed)
        .num_hours()
        .max(0) as f64;
    // Decay: 0.2 for very recent, approaching 0 after 24h
    let recency_bonus = 0.2 * (1.0 - (age_hours / 24.0).min(1.0));
    score += recency_bonus;

    score
}

/// Check if two task types are related (heuristic).
fn is_related_task(existing: &str, requested: &str) -> bool {
    const RELATED_PAIRS: &[(&str, &str)] = &[
        ("review", "fix"),
        ("fix", "review"),
        ("review", "refactor"),
        ("refactor", "review"),
        ("implement", "test"),
        ("test", "implement"),
        ("debug", "fix"),
        ("fix", "debug"),
    ];

    RELATED_PAIRS
        .iter()
        .any(|(a, b)| *a == existing && *b == requested)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_related_task_pairs() {
        assert!(is_related_task("review", "fix"));
        assert!(is_related_task("fix", "review"));
        assert!(is_related_task("debug", "fix"));
        assert!(!is_related_task("review", "deploy"));
        assert!(!is_related_task("implement", "deploy"));
    }

    #[test]
    fn test_relevance_score_base() {
        let session = MetaSessionState {
            meta_session_id: "test".to_string(),
            description: None,
            project_path: "/tmp".to_string(),
            created_at: chrono::Utc::now(),
            last_accessed: chrono::Utc::now(),
            genealogy: Default::default(),
            tools: Default::default(),
            context_status: Default::default(),
            total_token_usage: None,
            phase: csa_session::state::SessionPhase::Available,
            task_context: Default::default(),
        };

        let score = compute_relevance_score(&session, "default", "gemini-cli");
        // Base (0.3) + recency (~0.2 for just-now)
        assert!(score > 0.4 && score < 0.6, "score was {}", score);
    }

    #[test]
    fn test_relevance_score_exact_task_match() {
        let session = MetaSessionState {
            meta_session_id: "test".to_string(),
            description: None,
            project_path: "/tmp".to_string(),
            created_at: chrono::Utc::now(),
            last_accessed: chrono::Utc::now(),
            genealogy: Default::default(),
            tools: Default::default(),
            context_status: Default::default(),
            total_token_usage: None,
            phase: csa_session::state::SessionPhase::Available,
            task_context: csa_session::state::TaskContext {
                task_type: Some("review".to_string()),
                tier_name: None,
            },
        };

        let score = compute_relevance_score(&session, "review", "gemini-cli");
        // Base (0.3) + exact match (1.0) + recency (~0.2)
        assert!(score > 1.4, "score was {}", score);
    }
}
