use anyhow::{Result, anyhow};
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::Path;
use tracing::{info, warn};

use csa_session::{
    MetaSessionState, SessionPhase, list_sessions_from_root, list_sessions_from_root_readonly,
};

use super::RUNTIME_DIR_NAME;
use super::auto_gc::discover_project_roots;

pub(crate) const AUTO_GC_REAP_RUNTIME_MAX_AGE_DAYS: u64 = 30;

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub(crate) struct RuntimeReapStats {
    pub(crate) sessions_reaped: u64,
    pub(crate) bytes_reclaimed: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) entries: Vec<RuntimeReapEntry>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct RuntimeReapEntry {
    pub(crate) session_id: String,
    pub(crate) runtime_path: String,
    pub(crate) bytes_reclaimed: u64,
}

pub(super) fn require_runtime_reap_max_age(max_age_days: Option<u64>) -> Result<u64> {
    max_age_days.ok_or_else(|| anyhow!("`csa gc --reap-runtime` requires `--max-age-days <N>`"))
}

pub(crate) fn reap_runtime_payloads_global(
    dry_run: bool,
    max_age_days: u64,
) -> Result<RuntimeReapStats> {
    let state_bases = csa_config::paths::state_dir_all_roots();
    if state_bases.is_empty() {
        return Ok(RuntimeReapStats::default());
    }

    let current_session_id = std::env::var("CSA_SESSION_ID").ok();
    let mut total = RuntimeReapStats::default();
    let mut project_roots = Vec::new();
    for state_base in &state_bases {
        project_roots.extend(discover_project_roots(state_base));
    }

    for session_root in &project_roots {
        let sessions = match if dry_run {
            list_sessions_from_root_readonly(session_root)
        } else {
            list_sessions_from_root(session_root)
        } {
            Ok(sessions) => sessions,
            Err(err) => {
                warn!(
                    path = %session_root.display(),
                    error = %err,
                    "Failed to enumerate sessions for runtime reap; skipping project"
                );
                continue;
            }
        };
        let project_stats = match reap_runtime_payloads_in_root(
            session_root,
            &sessions,
            dry_run,
            max_age_days,
            current_session_id.as_deref(),
        ) {
            Ok(stats) => stats,
            Err(err) => {
                warn!(
                    path = %session_root.display(),
                    error = %err,
                    "Failed to reap runtime payloads for project; skipping project"
                );
                continue;
            }
        };
        merge_runtime_reap_stats(&mut total, project_stats);
    }

    Ok(total)
}

pub(super) fn reap_runtime_payloads_in_root(
    session_root: &Path,
    sessions: &[MetaSessionState],
    dry_run: bool,
    max_age_days: u64,
    current_session_id: Option<&str>,
) -> Result<RuntimeReapStats> {
    let now = chrono::Utc::now();
    let mut stats = RuntimeReapStats::default();

    for session in sessions {
        if current_session_id.is_some_and(|current| current == session.meta_session_id) {
            continue;
        }
        if !matches!(session.phase, SessionPhase::Retired) {
            continue;
        }

        let age = now.signed_duration_since(session.last_accessed);
        if age.num_days() < max_age_days as i64 {
            continue;
        }

        let session_dir = session_root.join("sessions").join(&session.meta_session_id);
        if session_has_live_lock(&session_dir)? {
            info!(
                session = %session.meta_session_id,
                "Skipping runtime reap for locked session"
            );
            continue;
        }

        let runtime_dir = session_dir.join(RUNTIME_DIR_NAME);
        let metadata = match fs::symlink_metadata(&runtime_dir) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                warn!(
                    session = %session.meta_session_id,
                    path = %runtime_dir.display(),
                    error = %err,
                    "Failed to inspect runtime directory; skipping"
                );
                continue;
            }
        };

        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            warn!(
                session = %session.meta_session_id,
                path = %runtime_dir.display(),
                "Skipping non-directory runtime path"
            );
            continue;
        }

        let bytes_reclaimed = match crate::preflight_state_dir::compute_state_dir_size(&runtime_dir)
        {
            Ok(bytes) => bytes,
            Err(err) => {
                warn!(
                    session = %session.meta_session_id,
                    path = %runtime_dir.display(),
                    error = %err,
                    "Failed to size runtime directory; skipping"
                );
                continue;
            }
        };

        if !dry_run {
            fs::remove_dir_all(&runtime_dir)?;
        }

        stats.sessions_reaped = stats.sessions_reaped.saturating_add(1);
        stats.bytes_reclaimed = stats.bytes_reclaimed.saturating_add(bytes_reclaimed);
        stats.entries.push(RuntimeReapEntry {
            session_id: session.meta_session_id.clone(),
            runtime_path: runtime_dir.display().to_string(),
            bytes_reclaimed,
        });
    }

    Ok(stats)
}

fn merge_runtime_reap_stats(total: &mut RuntimeReapStats, stats: RuntimeReapStats) {
    total.sessions_reaped = total.sessions_reaped.saturating_add(stats.sessions_reaped);
    total.bytes_reclaimed = total.bytes_reclaimed.saturating_add(stats.bytes_reclaimed);
    total.entries.extend(stats.entries);
}

fn session_has_live_lock(session_dir: &Path) -> Result<bool> {
    let locks_dir = session_dir.join("locks");
    if !locks_dir.is_dir() {
        return Ok(false);
    }

    for entry in fs::read_dir(&locks_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "lock") {
            continue;
        }
        if probe_lock_is_held(&path)? {
            return Ok(true);
        }
    }

    Ok(false)
}

fn probe_lock_is_held(lock_path: &Path) -> Result<bool> {
    let file = OpenOptions::new()
        .read(true)
        .open(lock_path)
        .map_err(|err| anyhow!("failed to open lock file {}: {err}", lock_path.display()))?;
    // SAFETY: `file.as_raw_fd()` is a valid descriptor for the opened lock file.
    // `LOCK_EX | LOCK_NB` performs the same non-blocking advisory-lock probe used
    // elsewhere in CSA session locking.
    let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if ret == 0 {
        // SAFETY: the fd above is still valid and currently holds the advisory lock;
        // unlocking before returning releases our probe lock immediately.
        unsafe {
            libc::flock(file.as_raw_fd(), libc::LOCK_UN);
        }
        return Ok(false);
    }

    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN => Ok(true),
        _ => Err(err.into()),
    }
}

pub(super) fn print_runtime_reap_summary(prefix: &str, stats: &RuntimeReapStats) {
    eprintln!(
        "{}  Runtime payloads reaped: {} ({})",
        prefix,
        stats.sessions_reaped,
        crate::session_cmds::format_file_size(stats.bytes_reclaimed)
    );
    for entry in &stats.entries {
        eprintln!(
            "{}    {}: {} ({})",
            prefix,
            entry.session_id,
            crate::session_cmds::format_file_size(entry.bytes_reclaimed),
            entry.runtime_path
        );
    }
}
