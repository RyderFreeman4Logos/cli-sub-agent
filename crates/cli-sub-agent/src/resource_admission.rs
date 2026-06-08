use std::path::Path;

use chrono::{DateTime, TimeDelta, Utc};
use csa_config::ProjectConfig;
use csa_resource::SpawnMemoryAdmission;
#[cfg(test)]
use csa_session::SandboxInfo;
use csa_session::{MetaSessionState, SessionPhase, SessionTreeMemorySampler};
use tracing::warn;

const FALLBACK_SPAWN_PROJECTION_MB: u64 = 4096;
const RECENT_ACTIVE_FALLBACK_PROJECTION_MB: u64 = 4096;
const RECENT_ACTIVE_FALLBACK_WINDOW_SECS: i64 = 15 * 60;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ActiveSessionMemory {
    active_count: u64,
    sampled_count: u64,
    sampled_rss_mb: u64,
    projected_mb: u64,
}

pub(crate) fn spawn_memory_projection_mb(config: Option<&ProjectConfig>, tool_name: &str) -> u64 {
    config
        .and_then(|cfg| cfg.sandbox_memory_max_mb(tool_name))
        .or_else(|| csa_config::default_sandbox_for_tool(tool_name).memory_max_mb)
        .unwrap_or(FALLBACK_SPAWN_PROJECTION_MB)
}

pub(crate) fn build_spawn_memory_admission(
    project_root: &Path,
    current_session_id: &str,
    projected_spawn_mb: u64,
) -> SpawnMemoryAdmission {
    let active = match csa_session::list_all_sessions_all_projects() {
        Ok(sessions) => aggregate_active_session_memory(
            &sessions,
            current_session_id,
            Utc::now(),
            sample_session_tree_rss_mb,
        ),
        Err(err) => {
            warn!(
                error = %err,
                project_root = %project_root.display(),
                "Failed to list active CSA sessions for host-memory admission"
            );
            ActiveSessionMemory::default()
        }
    };

    SpawnMemoryAdmission {
        projected_spawn_mb,
        active_session_rss_mb: active.sampled_rss_mb,
        active_session_projected_mb: active.projected_mb,
        active_session_count: active.active_count,
        sampled_session_count: active.sampled_count,
    }
}

fn sample_session_tree_rss_mb(session: &MetaSessionState) -> Option<u64> {
    let project_root = Path::new(&session.project_path);
    let sampler = SessionTreeMemorySampler::new(project_root, &session.meta_session_id).ok()?;
    sampler.sample_rss_mb().ok()
}

fn aggregate_active_session_memory(
    sessions: &[MetaSessionState],
    current_session_id: &str,
    now: DateTime<Utc>,
    sample_rss_mb: impl Fn(&MetaSessionState) -> Option<u64>,
) -> ActiveSessionMemory {
    let mut memory = ActiveSessionMemory::default();

    for session in sessions {
        if session.meta_session_id == current_session_id {
            continue;
        }
        if !matches!(session.phase, SessionPhase::Active) {
            continue;
        }

        memory.active_count = memory.active_count.saturating_add(1);

        let sampled_rss = sample_rss_mb(session);
        if let Some(rss_mb) = sampled_rss {
            memory.sampled_count = memory.sampled_count.saturating_add(1);
            memory.sampled_rss_mb = memory.sampled_rss_mb.saturating_add(rss_mb);
        }

        let sandbox_projection = session
            .sandbox_info
            .as_ref()
            .and_then(|sandbox| sandbox.memory_max_mb);
        let recent_pending_projection =
            recent_pending_projection_mb(session, now, sampled_rss, sandbox_projection);
        let session_projection = sampled_rss
            .unwrap_or(0)
            .max(sandbox_projection.unwrap_or(0))
            .max(recent_pending_projection);
        memory.projected_mb = memory.projected_mb.saturating_add(session_projection);
    }

    memory
}

fn recent_pending_projection_mb(
    session: &MetaSessionState,
    now: DateTime<Utc>,
    sampled_rss: Option<u64>,
    sandbox_projection: Option<u64>,
) -> u64 {
    if sampled_rss.is_some() || sandbox_projection.is_some() {
        return 0;
    }

    let age = now.signed_duration_since(session.last_accessed);
    if age <= TimeDelta::seconds(RECENT_ACTIVE_FALLBACK_WINDOW_SECS) {
        RECENT_ACTIVE_FALLBACK_PROJECTION_MB
    } else {
        0
    }
}

pub(crate) fn active_session_count_for_balloon(
    project_root: &Path,
    current_session_id: &str,
) -> u64 {
    match csa_session::list_all_sessions_all_projects() {
        Ok(sessions) => sessions
            .iter()
            .filter(|session| session.meta_session_id != current_session_id)
            .filter(|session| matches!(session.phase, SessionPhase::Active))
            .count()
            .try_into()
            .unwrap_or(u64::MAX),
        Err(err) => {
            warn!(
                error = %err,
                project_root = %project_root.display(),
                "Failed to count active CSA sessions for memory-balloon admission"
            );
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_session(
        id: &str,
        last_accessed: DateTime<Utc>,
        memory_max_mb: Option<u64>,
    ) -> MetaSessionState {
        MetaSessionState {
            meta_session_id: id.to_string(),
            phase: SessionPhase::Active,
            last_accessed,
            sandbox_info: Some(SandboxInfo {
                mode: "cgroup".to_string(),
                memory_max_mb,
                filesystem_mode: None,
                readonly_project_root: None,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn spawn_projection_uses_configured_tool_limit() {
        let cfg: ProjectConfig =
            toml::from_str("[resources]\nmemory_max_mb = 8192\n").expect("config should parse");

        assert_eq!(spawn_memory_projection_mb(Some(&cfg), "codex"), 8192);
    }

    #[test]
    fn spawn_projection_uses_tool_default_without_config() {
        assert_eq!(spawn_memory_projection_mb(None, "codex"), 12_288);
    }

    #[test]
    fn active_memory_uses_max_of_rss_and_sandbox_projection() {
        let now = Utc::now();
        let sessions = vec![
            active_session("current", now, Some(12_288)),
            active_session("a", now, Some(8192)),
            active_session("b", now, Some(2048)),
        ];

        let memory = aggregate_active_session_memory(&sessions, "current", now, |session| {
            match session.meta_session_id.as_str() {
                "a" => Some(1024),
                "b" => Some(4096),
                _ => None,
            }
        });

        assert_eq!(memory.active_count, 2);
        assert_eq!(memory.sampled_count, 2);
        assert_eq!(memory.sampled_rss_mb, 5120);
        assert_eq!(memory.projected_mb, 12_288);
    }

    #[test]
    fn active_memory_adds_recent_pending_projection_without_rss_or_sandbox() {
        let now = Utc::now();
        let sessions = vec![MetaSessionState {
            meta_session_id: "pending".to_string(),
            phase: SessionPhase::Active,
            last_accessed: now - TimeDelta::minutes(2),
            sandbox_info: None,
            ..Default::default()
        }];

        let memory = aggregate_active_session_memory(&sessions, "current", now, |_| None);

        assert_eq!(memory.active_count, 1);
        assert_eq!(memory.sampled_count, 0);
        assert_eq!(memory.projected_mb, RECENT_ACTIVE_FALLBACK_PROJECTION_MB);
    }

    #[test]
    fn active_memory_ignores_old_pending_session_without_rss_or_sandbox() {
        let now = Utc::now();
        let sessions = vec![MetaSessionState {
            meta_session_id: "old".to_string(),
            phase: SessionPhase::Active,
            last_accessed: now - TimeDelta::hours(2),
            sandbox_info: None,
            ..Default::default()
        }];

        let memory = aggregate_active_session_memory(&sessions, "current", now, |_| None);

        assert_eq!(memory.active_count, 1);
        assert_eq!(memory.projected_mb, 0);
    }
}
