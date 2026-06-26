use std::{io, path::Path};

use anyhow::Context;
use chrono::{DateTime, TimeDelta, Utc};
use csa_config::ProjectConfig;
use csa_resource::SpawnMemoryAdmission;
use csa_session::{MetaSessionState, SandboxInfo, SessionPhase, SessionTreeMemorySampler};
use tracing::warn;

use crate::run_resource_overrides::RunResourceOverrides;

const FALLBACK_SPAWN_PROJECTION_MB: u64 = 4096;
const RECENT_ACTIVE_FALLBACK_PROJECTION_MB: u64 = 4096;
const RECENT_ACTIVE_FALLBACK_WINDOW_SECS: i64 = 15 * 60;
const PRE_SPAWN_ADMISSION_MODE: &str = "admission";
const PRE_SPAWN_MEMORY_ADMITTED_FILE: &str = ".pre-spawn-memory-admitted.toml";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ActiveSessionMemory {
    active_count: u64,
    sampled_count: u64,
    sampled_rss_mb: u64,
    projected_mb: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActiveSessionObservation {
    sampled_rss_mb: Option<u64>,
    projected_mb: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionMemorySample {
    RssMb(u64),
    UnsupportedLiveProcess,
    Unavailable,
}

#[cfg(test)]
pub(crate) fn spawn_memory_projection_mb(config: Option<&ProjectConfig>, tool_name: &str) -> u64 {
    spawn_memory_projection_mb_with_overrides(config, tool_name, RunResourceOverrides::default())
}

pub(crate) fn spawn_memory_projection_mb_with_overrides(
    config: Option<&ProjectConfig>,
    tool_name: &str,
    resource_overrides: RunResourceOverrides,
) -> u64 {
    resource_overrides
        .resolve_memory_max_mb(config, tool_name)
        .unwrap_or(FALLBACK_SPAWN_PROJECTION_MB)
}

pub(crate) fn persist_spawn_memory_projection(
    session: &mut MetaSessionState,
    projected_spawn_mb: u64,
) -> anyhow::Result<()> {
    if update_spawn_memory_projection(session, projected_spawn_mb) {
        csa_session::save_session(session)?;
    }
    Ok(())
}

pub(crate) fn spawn_memory_admission_ready_path(session_dir: &Path) -> std::path::PathBuf {
    session_dir.join(PRE_SPAWN_MEMORY_ADMITTED_FILE)
}

pub(crate) fn persist_spawn_memory_admission_ready(
    project_root: &Path,
    session_id: &str,
    projected_spawn_mb: u64,
) -> anyhow::Result<()> {
    let session_dir = csa_session::get_session_dir(project_root, session_id)?;
    let path = spawn_memory_admission_ready_path(&session_dir);
    let content = format!(
        "status = \"admitted\"\nprojected_spawn_mb = {projected_spawn_mb}\nrecorded_at = \"{}\"\n",
        Utc::now().to_rfc3339()
    );
    std::fs::write(&path, content).with_context(|| {
        format!(
            "Failed to persist pre-spawn memory admission marker at {}",
            path.display()
        )
    })
}

fn update_spawn_memory_projection(session: &mut MetaSessionState, projected_spawn_mb: u64) -> bool {
    session.sandbox_info = Some(SandboxInfo {
        mode: PRE_SPAWN_ADMISSION_MODE.to_string(),
        memory_max_mb: Some(projected_spawn_mb),
        filesystem_mode: None,
        readonly_project_root: None,
    });
    session.last_accessed = Utc::now();
    true
}

pub(crate) fn clear_spawn_memory_projection(session: &mut MetaSessionState) -> bool {
    if session
        .sandbox_info
        .as_ref()
        .is_none_or(|info| info.mode != PRE_SPAWN_ADMISSION_MODE)
    {
        return false;
    }

    session.sandbox_info = None;
    true
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
            sample_session_memory,
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

fn sample_session_memory(session: &MetaSessionState) -> SessionMemorySample {
    let project_root = Path::new(&session.project_path);
    match SessionTreeMemorySampler::new(project_root, &session.meta_session_id)
        .and_then(|sampler| sampler.sample_rss_mb())
    {
        Ok(rss_mb) => SessionMemorySample::RssMb(rss_mb),
        Err(err)
            if err.kind() == io::ErrorKind::Unsupported
                && session_has_live_process_signal(project_root, &session.meta_session_id) =>
        {
            SessionMemorySample::UnsupportedLiveProcess
        }
        Err(_) => SessionMemorySample::Unavailable,
    }
}

fn session_has_live_process_signal(project_root: &Path, session_id: &str) -> bool {
    let Ok(session_dir) = csa_session::get_session_dir(project_root, session_id) else {
        return false;
    };

    csa_process::ToolLiveness::has_live_process(&session_dir)
        || csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir)
}

fn aggregate_active_session_memory(
    sessions: &[MetaSessionState],
    current_session_id: &str,
    now: DateTime<Utc>,
    sample_memory: impl Fn(&MetaSessionState) -> SessionMemorySample,
) -> ActiveSessionMemory {
    let mut memory = ActiveSessionMemory::default();

    for session in sessions {
        if session.meta_session_id == current_session_id {
            continue;
        }
        if !matches!(session.phase, SessionPhase::Active) {
            continue;
        }

        let Some(observation) = active_session_observation(session, now, &sample_memory) else {
            continue;
        };

        memory.active_count = memory.active_count.saturating_add(1);

        if let Some(rss_mb) = observation.sampled_rss_mb {
            memory.sampled_count = memory.sampled_count.saturating_add(1);
            memory.sampled_rss_mb = memory.sampled_rss_mb.saturating_add(rss_mb);
        }

        memory.projected_mb = memory.projected_mb.saturating_add(observation.projected_mb);
    }

    memory
}

fn active_session_observation(
    session: &MetaSessionState,
    now: DateTime<Utc>,
    sample_memory: impl Fn(&MetaSessionState) -> SessionMemorySample,
) -> Option<ActiveSessionObservation> {
    let sample = sample_memory(session);
    let sandbox_projection = session
        .sandbox_info
        .as_ref()
        .and_then(|sandbox| sandbox.memory_max_mb);

    match sample {
        SessionMemorySample::RssMb(rss_mb) => {
            return Some(ActiveSessionObservation {
                sampled_rss_mb: Some(rss_mb),
                projected_mb: rss_mb.max(sandbox_projection.unwrap_or(0)),
            });
        }
        SessionMemorySample::UnsupportedLiveProcess => {
            return Some(ActiveSessionObservation {
                sampled_rss_mb: None,
                projected_mb: sandbox_projection.unwrap_or(FALLBACK_SPAWN_PROJECTION_MB),
            });
        }
        SessionMemorySample::Unavailable => {}
    }

    let recent_pending_projection = recent_pending_projection_mb(session, now, sandbox_projection);
    (recent_pending_projection > 0).then_some(ActiveSessionObservation {
        sampled_rss_mb: None,
        projected_mb: recent_pending_projection,
    })
}

fn recent_pending_projection_mb(
    session: &MetaSessionState,
    now: DateTime<Utc>,
    sandbox_projection: Option<u64>,
) -> u64 {
    if session
        .sandbox_info
        .as_ref()
        .is_none_or(|sandbox| sandbox.mode != PRE_SPAWN_ADMISSION_MODE)
    {
        return 0;
    }

    let age = now.signed_duration_since(session.last_accessed);
    if age <= TimeDelta::seconds(RECENT_ACTIVE_FALLBACK_WINDOW_SECS) {
        sandbox_projection.unwrap_or(RECENT_ACTIVE_FALLBACK_PROJECTION_MB)
    } else {
        0
    }
}

pub(crate) fn active_session_count_for_balloon(
    project_root: &Path,
    current_session_id: &str,
) -> u64 {
    match csa_session::list_all_sessions_all_projects() {
        Ok(sessions) => count_observable_active_sessions(
            &sessions,
            current_session_id,
            Utc::now(),
            sample_session_memory,
        ),
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

fn count_observable_active_sessions(
    sessions: &[MetaSessionState],
    current_session_id: &str,
    now: DateTime<Utc>,
    sample_memory: impl Fn(&MetaSessionState) -> SessionMemorySample,
) -> u64 {
    sessions
        .iter()
        .filter(|session| session.meta_session_id != current_session_id)
        .filter(|session| matches!(session.phase, SessionPhase::Active))
        .filter(|session| active_session_observation(session, now, &sample_memory).is_some())
        .count()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_session(
        id: &str,
        last_accessed: DateTime<Utc>,
        memory_max_mb: Option<u64>,
    ) -> MetaSessionState {
        active_session_with_mode(id, last_accessed, "cgroup", memory_max_mb)
    }

    fn active_session_with_mode(
        id: &str,
        last_accessed: DateTime<Utc>,
        mode: &str,
        memory_max_mb: Option<u64>,
    ) -> MetaSessionState {
        MetaSessionState {
            meta_session_id: id.to_string(),
            phase: SessionPhase::Active,
            last_accessed,
            sandbox_info: Some(SandboxInfo {
                mode: mode.to_string(),
                memory_max_mb,
                filesystem_mode: None,
                readonly_project_root: None,
            }),
            ..Default::default()
        }
    }

    fn admission_session(
        id: &str,
        last_accessed: DateTime<Utc>,
        memory_max_mb: Option<u64>,
    ) -> MetaSessionState {
        MetaSessionState {
            meta_session_id: id.to_string(),
            phase: SessionPhase::Active,
            last_accessed,
            sandbox_info: Some(SandboxInfo {
                mode: PRE_SPAWN_ADMISSION_MODE.to_string(),
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
    fn spawn_projection_uses_run_override_before_tool_config() {
        let cfg: ProjectConfig = toml::from_str(
            r#"
[tools.codex]
memory_max_mb = 16384
"#,
        )
        .expect("config should parse");
        let overrides = RunResourceOverrides::new(Some(6144), None);

        assert_eq!(
            spawn_memory_projection_mb_with_overrides(Some(&cfg), "codex", overrides),
            6144
        );
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
                "a" => SessionMemorySample::RssMb(1024),
                "b" => SessionMemorySample::RssMb(4096),
                _ => SessionMemorySample::Unavailable,
            }
        });

        assert_eq!(memory.active_count, 2);
        assert_eq!(memory.sampled_count, 2);
        assert_eq!(memory.sampled_rss_mb, 5120);
        assert_eq!(memory.projected_mb, 12_288);
    }

    #[test]
    fn active_memory_adds_recent_pending_sandbox_projection_without_rss() {
        let now = Utc::now();
        let sessions = vec![admission_session(
            "pending",
            now - TimeDelta::minutes(2),
            Some(12_288),
        )];

        let memory = aggregate_active_session_memory(&sessions, "current", now, |_| {
            SessionMemorySample::Unavailable
        });

        assert_eq!(memory.active_count, 1);
        assert_eq!(memory.sampled_count, 0);
        assert_eq!(memory.projected_mb, 12_288);
    }

    #[test]
    fn active_memory_uses_fallback_for_recent_pending_without_sandbox_projection() {
        let now = Utc::now();
        let sessions = vec![admission_session(
            "pending",
            now - TimeDelta::minutes(2),
            None,
        )];

        let memory = aggregate_active_session_memory(&sessions, "current", now, |_| {
            SessionMemorySample::Unavailable
        });

        assert_eq!(memory.active_count, 1);
        assert_eq!(memory.sampled_count, 0);
        assert_eq!(memory.projected_mb, RECENT_ACTIVE_FALLBACK_PROJECTION_MB);
    }

    #[test]
    fn active_memory_ignores_recent_runtime_session_without_live_signal() {
        let now = Utc::now();
        let sessions = vec![active_session(
            "completed",
            now - TimeDelta::minutes(2),
            Some(12_288),
        )];

        let memory = aggregate_active_session_memory(&sessions, "current", now, |_| {
            SessionMemorySample::Unavailable
        });

        assert_eq!(memory.active_count, 0);
        assert_eq!(memory.sampled_count, 0);
        assert_eq!(memory.projected_mb, 0);
    }

    #[test]
    fn active_memory_ignores_old_pending_session_without_live_signal() {
        let now = Utc::now();
        let sessions = vec![admission_session(
            "old",
            now - TimeDelta::hours(2),
            Some(12_288),
        )];

        let memory = aggregate_active_session_memory(&sessions, "current", now, |_| {
            SessionMemorySample::Unavailable
        });

        assert_eq!(memory.active_count, 0);
        assert_eq!(memory.projected_mb, 0);
    }

    #[test]
    fn active_memory_counts_old_session_with_live_sample() {
        let now = Utc::now();
        let sessions = vec![active_session(
            "live",
            now - TimeDelta::hours(2),
            Some(12_288),
        )];

        let memory = aggregate_active_session_memory(&sessions, "current", now, |_| {
            SessionMemorySample::RssMb(1024)
        });

        assert_eq!(memory.active_count, 1);
        assert_eq!(memory.sampled_count, 1);
        assert_eq!(memory.sampled_rss_mb, 1024);
        assert_eq!(memory.projected_mb, 12_288);
    }

    #[test]
    fn active_memory_counts_live_runtime_projection_when_sampler_unsupported() {
        let now = Utc::now();
        let sessions = vec![active_session_with_mode(
            "live",
            now - TimeDelta::hours(2),
            "rlimit",
            Some(12_288),
        )];

        let memory = aggregate_active_session_memory(&sessions, "current", now, |_| {
            SessionMemorySample::UnsupportedLiveProcess
        });

        assert_eq!(memory.active_count, 1);
        assert_eq!(memory.sampled_count, 0);
        assert_eq!(memory.sampled_rss_mb, 0);
        assert_eq!(memory.projected_mb, 12_288);
    }

    #[test]
    fn active_memory_uses_fallback_for_live_runtime_without_projection_when_sampler_unsupported() {
        let now = Utc::now();
        let sessions = vec![active_session_with_mode(
            "live",
            now - TimeDelta::hours(2),
            "none",
            None,
        )];

        let memory = aggregate_active_session_memory(&sessions, "current", now, |_| {
            SessionMemorySample::UnsupportedLiveProcess
        });

        assert_eq!(memory.active_count, 1);
        assert_eq!(memory.sampled_count, 0);
        assert_eq!(memory.projected_mb, FALLBACK_SPAWN_PROJECTION_MB);
    }

    #[test]
    fn balloon_count_ignores_stale_active_without_live_signal() {
        let now = Utc::now();
        let sessions = vec![
            admission_session("recent", now - TimeDelta::minutes(2), Some(4096)),
            admission_session("old", now - TimeDelta::hours(2), Some(4096)),
        ];

        let count = count_observable_active_sessions(&sessions, "current", now, |_| {
            SessionMemorySample::Unavailable
        });

        assert_eq!(count, 1);
    }

    #[test]
    fn balloon_count_ignores_recent_runtime_active_without_live_signal() {
        let now = Utc::now();
        let sessions = vec![active_session(
            "completed",
            now - TimeDelta::minutes(2),
            Some(4096),
        )];

        let count = count_observable_active_sessions(&sessions, "current", now, |_| {
            SessionMemorySample::Unavailable
        });

        assert_eq!(count, 0);
    }

    #[test]
    fn balloon_count_counts_live_runtime_when_sampler_unsupported() {
        let now = Utc::now();
        let sessions = vec![active_session_with_mode(
            "live",
            now - TimeDelta::hours(2),
            "rlimit",
            Some(4096),
        )];

        let count = count_observable_active_sessions(&sessions, "current", now, |_| {
            SessionMemorySample::UnsupportedLiveProcess
        });

        assert_eq!(count, 1);
    }

    #[test]
    fn spawn_projection_update_records_pre_spawn_memory_limit() {
        let mut session = MetaSessionState::default();

        assert!(update_spawn_memory_projection(&mut session, 12_288));
        let info = session.sandbox_info.expect("projection should be recorded");

        assert_eq!(info.mode, PRE_SPAWN_ADMISSION_MODE);
        assert_eq!(info.memory_max_mb, Some(12_288));
        assert_eq!(info.filesystem_mode, None);
        assert_eq!(info.readonly_project_root, None);
    }

    #[test]
    fn spawn_projection_update_refreshes_existing_projection_timestamp() {
        let old = Utc::now() - TimeDelta::hours(2);
        let mut session = active_session("current", old, Some(12_288));
        session.last_accessed = old;

        assert!(update_spawn_memory_projection(&mut session, 12_288));

        assert!(
            session.last_accessed > old,
            "unchanged pre-spawn projection must still refresh the pending-start window"
        );
        let info = session
            .sandbox_info
            .as_ref()
            .expect("projection should stay recorded");
        assert_eq!(info.mode, PRE_SPAWN_ADMISSION_MODE);
        assert_eq!(info.filesystem_mode, None);
        assert_eq!(info.readonly_project_root, None);
    }

    #[test]
    fn clear_spawn_projection_removes_admission_marker() {
        let now = Utc::now();
        let mut session = admission_session("pending", now, Some(12_288));

        assert!(clear_spawn_memory_projection(&mut session));

        assert_eq!(session.sandbox_info, None);
    }

    #[test]
    fn clear_spawn_projection_preserves_runtime_sandbox_info() {
        let now = Utc::now();
        let mut session = active_session("completed", now, Some(12_288));

        assert!(!clear_spawn_memory_projection(&mut session));

        assert_eq!(
            session.sandbox_info.as_ref().map(|info| info.mode.as_str()),
            Some("cgroup")
        );
    }
}
