use anyhow::Result;
use std::fs;
use std::path::Path;
use tracing::{info, warn};

use csa_config::{GcConfig, GlobalConfig};
use csa_core::types::OutputFormat;
use csa_resource::cleanup_orphan_scopes;
use csa_session::{
    PhaseEvent, delete_session, get_session_dir, get_session_root, list_sessions, save_session_in,
};

mod auto_gc;
#[path = "gc_args.rs"]
mod gc_args;
mod reaper;
mod transcript;

#[cfg(test)]
use auto_gc::discover_project_roots;
pub(crate) use auto_gc::{handle_gc_global, invalidate_state_dir_size_cache};
pub use gc_args::GcArgs;
pub(crate) use reaper::AUTO_GC_REAP_RUNTIME_MAX_AGE_DAYS;
use reaper::{
    print_runtime_reap_summary, reap_runtime_payloads_in_root, require_runtime_reap_max_age,
};
use transcript::{cleanup_project_transcripts, load_gc_config_for_sessions};

/// Default age threshold (in days) for retiring stale Active sessions.
const RETIRE_AFTER_DAYS: i64 = 7;
const STATE_DIR_SIZE_CACHE_FILENAME: &str = ".size-cache.toml";
const RUNTIME_DIR_NAME: &str = "runtime";

pub(crate) type RuntimeReapStats = reaper::RuntimeReapStats;

pub(crate) fn reap_runtime_payloads_global(
    dry_run: bool,
    max_age_days: u64,
) -> Result<RuntimeReapStats> {
    reaper::reap_runtime_payloads_global(dry_run, max_age_days)
}

pub(crate) fn handle_gc(
    dry_run: bool,
    max_age_days: Option<u64>,
    reap_runtime: bool,
    format: OutputFormat,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(None)?;
    let session_root = get_session_root(&project_root)?;
    let sessions = list_sessions(&project_root, None)?;
    let gc_config = GcConfig::load_for_project(&project_root)?;
    let now = chrono::Utc::now();
    let runtime_reap_max_age_days =
        runtime_reap_max_age_days(reap_runtime, max_age_days, gc_config)?;

    let mut stale_locks_removed = 0;
    let mut empty_sessions_removed = 0;
    let mut orphan_dirs_removed = 0;
    let mut expired_sessions_removed = 0;
    let mut sessions_retired = 0u64;
    let mut orphan_scopes_cleaned = 0u64;

    if dry_run {
        eprintln!("[dry-run] No changes will be made.");
    }

    for session in &sessions {
        let session_dir = get_session_dir(&project_root, &session.meta_session_id)?;
        let locks_dir = session_dir.join("locks");

        if locks_dir.exists()
            && let Ok(entries) = fs::read_dir(&locks_dir)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "lock")
                    && let Ok(content) = fs::read_to_string(&path)
                    && let Some(pid) = extract_pid_from_lock(&content)
                    && !is_process_alive(pid)
                {
                    if dry_run {
                        eprintln!(
                            "[dry-run] Would remove stale lock for dead PID {}: {:?}",
                            pid,
                            path.file_name()
                        );
                        stale_locks_removed += 1;
                    } else if fs::remove_file(&path).is_ok() {
                        info!(
                            "Removed stale lock for dead PID {}: {:?}",
                            pid,
                            path.file_name()
                        );
                        stale_locks_removed += 1;
                    }
                }
            }
        }

        if session.tools.is_empty() {
            if dry_run {
                eprintln!(
                    "[dry-run] Would remove empty session: {}",
                    session.meta_session_id
                );
                empty_sessions_removed += 1;
            } else if delete_session(&project_root, &session.meta_session_id).is_ok() {
                empty_sessions_removed += 1;
            }
            continue;
        }

        // Retire stale Active/Available sessions (>7 days since last access)
        let age = now.signed_duration_since(session.last_accessed);
        if age.num_days() > RETIRE_AFTER_DAYS
            && session.phase.transition(&PhaseEvent::Retired).is_ok()
        {
            if dry_run {
                eprintln!(
                    "[dry-run] Would retire stale session: {} (phase={}, {} days old)",
                    session.meta_session_id,
                    session.phase,
                    age.num_days()
                );
                sessions_retired += 1;
            } else {
                let mut updated = session.clone();
                match updated.phase.transition(&PhaseEvent::Retired) {
                    Ok(new_phase) => {
                        updated.phase = new_phase;
                        match save_session_in(&session_root, &updated) {
                            Ok(_) => {
                                info!(
                                    session = %session.meta_session_id,
                                    age_days = age.num_days(),
                                    "Retired stale session"
                                );
                                sessions_retired += 1;
                            }
                            Err(e) => {
                                warn!(
                                    session = %session.meta_session_id,
                                    error = %e,
                                    "Failed to persist retirement"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            session = %session.meta_session_id,
                            error = %e,
                            "Skipping retirement"
                        );
                    }
                }
            }
        }

        if !reap_runtime
            && let Some(days) = max_age_days
            && age.num_days() > days as i64
        {
            if dry_run {
                eprintln!(
                    "[dry-run] Would remove expired session: {} (last accessed {} days ago)",
                    session.meta_session_id,
                    age.num_days()
                );
                expired_sessions_removed += 1;
            } else if delete_session(&project_root, &session.meta_session_id).is_ok() {
                info!("Removed expired session: {}", session.meta_session_id);
                expired_sessions_removed += 1;
            }
        }
    }

    let session_root = csa_session::get_session_root(&project_root)?;
    let rotation_path = session_root.join("rotation.toml");
    if rotation_path.exists() {
        let remaining = list_sessions(&project_root, None)?;
        if remaining.is_empty() {
            if dry_run {
                eprintln!("[dry-run] Would remove rotation state: {rotation_path:?}");
            } else if fs::remove_file(&rotation_path).is_ok() {
                info!("Removed rotation state (no sessions remain)");
            }
        }
    }

    let sessions_dir = session_root.join("sessions");

    if sessions_dir.exists()
        && let Ok(entries) = fs::read_dir(&sessions_dir)
    {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|ft| ft.is_dir()) && is_orphan_session_dir(&entry) {
                let session_dir = entry.path();
                if dry_run {
                    eprintln!(
                        "[dry-run] Would remove orphan directory: {}",
                        session_dir.display()
                    );
                    orphan_dirs_removed += 1;
                } else if fs::remove_dir_all(&session_dir).is_ok() {
                    info!(
                        "Removed orphan directory without state.toml: {}",
                        session_dir.display()
                    );
                    orphan_dirs_removed += 1;
                }
            }
        }
    }

    let current_session_id = std::env::var("CSA_SESSION_ID").ok();
    let runtime_reap_stats = runtime_reap_max_age_days
        .map(|days| {
            let sessions_for_reap = if dry_run {
                sessions.clone()
            } else {
                list_sessions(&project_root, None)?
            };
            reap_runtime_payloads_in_root(
                &session_root,
                &sessions_for_reap,
                dry_run,
                days,
                current_session_id.as_deref(),
            )
        })
        .transpose()?;

    let transcript_stats = cleanup_project_transcripts(&session_root, gc_config, dry_run);

    if dry_run {
        eprintln!("[dry-run] Would scan for orphan csa-*.scope units with 0 active PIDs");
    } else {
        match cleanup_orphan_scopes() {
            Ok(cleaned) => {
                orphan_scopes_cleaned = cleaned.len() as u64;
                for scope in cleaned {
                    info!(
                        scope = %scope.unit_name,
                        active_pids = scope.active_pids,
                        "Stopped orphan cgroup scope (stale unit with no live processes)"
                    );
                }
            }
            Err(err) => {
                warn!(error = %err, "Failed to enumerate orphan cgroup scopes; skipping");
            }
        }
    }

    let mut stale_slots_cleaned = 0;
    if let Ok(slots_dir) = GlobalConfig::slots_dir()
        && slots_dir.exists()
        && let Ok(entries) = fs::read_dir(&slots_dir)
    {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|ft| ft.is_dir()) {
                let tool_dir = entry.path();
                if let Ok(slot_entries) = fs::read_dir(&tool_dir) {
                    for slot_entry in slot_entries.flatten() {
                        let path = slot_entry.path();
                        if path.extension().is_some_and(|ext| ext == "lock")
                            && let Ok(content) = fs::read_to_string(&path)
                            && let Some(pid) = extract_pid_from_lock(&content)
                            && !is_process_alive(pid)
                        {
                            if dry_run {
                                eprintln!(
                                    "[dry-run] Would clean stale slot: {:?} (dead PID {})",
                                    path.file_name(),
                                    pid
                                );
                                stale_slots_cleaned += 1;
                            } else if fs::remove_file(&path).is_ok() {
                                info!(
                                    "Cleaned stale slot: {:?} (dead PID {})",
                                    path.file_name(),
                                    pid
                                );
                                stale_slots_cleaned += 1;
                            }
                        }
                    }
                }
                if !dry_run {
                    let _ = fs::remove_dir(&tool_dir); // only succeeds if empty
                }
            }
        }
    }

    match format {
        OutputFormat::Json => {
            let mut summary = serde_json::json!({
                "dry_run": dry_run,
                "stale_locks_removed": stale_locks_removed,
                "empty_sessions_removed": empty_sessions_removed,
                "orphan_dirs_removed": orphan_dirs_removed,
                "sessions_retired": sessions_retired,
                "transcripts_removed": transcript_stats.files_removed,
                "transcript_bytes_reclaimed": transcript_stats.bytes_reclaimed,
                "stale_slots_cleaned": stale_slots_cleaned,
                "orphan_scopes_cleaned": orphan_scopes_cleaned,
            });
            if !reap_runtime && max_age_days.is_some() {
                summary["expired_sessions_removed"] = serde_json::json!(expired_sessions_removed);
            }
            if let Some(runtime_reap_stats) = runtime_reap_stats.as_ref() {
                summary["runtime_reap"] = serde_json::to_value(runtime_reap_stats)?;
            }
            println!("{}", serde_json::to_string_pretty(&summary)?);
        }
        OutputFormat::Text => {
            let prefix = if dry_run { "[dry-run] " } else { "" };
            eprintln!(
                "{}Garbage collection {}:",
                prefix,
                if dry_run { "preview" } else { "complete" }
            );
            eprintln!("{prefix}  Stale locks removed: {stale_locks_removed}");
            eprintln!("{prefix}  Empty sessions removed: {empty_sessions_removed}");
            if sessions_retired > 0 {
                eprintln!("{prefix}  Sessions retired: {sessions_retired}");
            }
            if !reap_runtime && max_age_days.is_some() {
                eprintln!("{prefix}  Expired sessions removed: {expired_sessions_removed}");
            }
            if let Some(runtime_reap_stats) = runtime_reap_stats.as_ref() {
                print_runtime_reap_summary(prefix, runtime_reap_stats);
            }
            eprintln!("{prefix}  Orphan directories removed: {orphan_dirs_removed}");
            eprintln!(
                "{}  Transcript files removed: {} ({} bytes)",
                prefix, transcript_stats.files_removed, transcript_stats.bytes_reclaimed
            );
            eprintln!("{prefix}  Stale slots cleaned: {stale_slots_cleaned}");
            eprintln!("{prefix}  Orphan cgroup scopes cleaned: {orphan_scopes_cleaned}");
        }
    }

    if !dry_run {
        invalidate_state_dir_size_cache();
    }

    Ok(())
}

pub(super) fn runtime_reap_max_age_days(
    reap_runtime: bool,
    max_age_days: Option<u64>,
    gc_config: GcConfig,
) -> Result<Option<u64>> {
    if reap_runtime {
        return require_runtime_reap_max_age(max_age_days).map(Some);
    }
    Ok(gc_config
        .reap_runtime_dirs
        .then_some(RETIRE_AFTER_DAYS as u64))
}

/// Extract PID from lock file JSON content (expects `{"pid": N}`).
fn extract_pid_from_lock(json_content: &str) -> Option<u32> {
    let v: serde_json::Value = serde_json::from_str(json_content).ok()?;
    let n = v.get("pid")?.as_u64()?;
    u32::try_from(n).ok()
}

/// Returns `true` for valid-ULID non-hidden dirs in `sessions/` lacking `state.toml`.
fn is_orphan_session_dir(entry: &fs::DirEntry) -> bool {
    let name = entry.file_name();
    let name_str = name.to_string_lossy();
    if name_str.starts_with('.') {
        return false;
    }
    // Only valid ULID dirs can be orphan sessions (strict format, not just length).
    if csa_session::validate_session_id(&name_str).is_err() {
        return false;
    }
    let path = entry.path();
    if path.join("state.toml").exists() {
        return false;
    }
    if path.join("sessions").is_dir() {
        return false;
    }
    true
}

/// Check if a directory has ULID subdirs with `state.toml` (confirmed session container).
/// Used for recursion skip: only skip traversal into confirmed session containers.
/// Path-segment "sessions/" dirs whose ULID children lack `state.toml` are traversed.
fn has_confirmed_sessions(dir: &Path) -> bool {
    fs::read_dir(dir).is_ok_and(|rd| {
        rd.flatten().any(|e| {
            e.file_type().is_ok_and(|ft| ft.is_dir())
                && csa_session::validate_session_id(&e.file_name().to_string_lossy()).is_ok()
                && e.path().join("state.toml").exists()
        })
    })
}

/// Check if a process is alive (cross-platform Unix).
fn is_process_alive(pid: u32) -> bool {
    // SAFETY: signal 0 checks existence without sending a signal.
    // EPERM means process exists but we lack permission to signal it.
    let ret = unsafe { libc::kill(pid as libc::pid_t, 0) };
    ret == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
#[path = "gc_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "gc_runtime_tests.rs"]
mod runtime_tests;
