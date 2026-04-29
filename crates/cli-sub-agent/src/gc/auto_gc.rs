use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

use csa_config::GlobalConfig;
use csa_core::types::OutputFormat;
use csa_resource::cleanup_orphan_scopes;
use csa_session::{list_sessions_from_root, list_sessions_from_root_readonly, save_session_in};

use super::load_gc_config_for_sessions;
use super::reaper::{
    merge_runtime_reap_stats, print_runtime_reap_summary, reap_runtime_payloads_in_root,
    sessions_with_dry_run_retirements, stale_session_retirement_candidate,
};
use super::{
    RETIRE_AFTER_DAYS, STATE_DIR_SIZE_CACHE_FILENAME, extract_pid_from_lock,
    has_confirmed_sessions, is_orphan_session_dir, is_process_alive, runtime_reap_max_age_days,
};

pub(crate) fn handle_gc_global(
    dry_run: bool,
    max_age_days: Option<u64>,
    reap_runtime: bool,
    format: OutputFormat,
) -> Result<()> {
    let state_bases = csa_config::paths::state_dir_all_roots();
    if state_bases.is_empty() {
        let state_base = GlobalConfig::state_base_dir()?;
        eprintln!("No CSA state directory found at {}", state_base.display());
        return Ok(());
    }

    let mut project_roots = Vec::new();
    for state_base in &state_bases {
        project_roots.extend(discover_project_roots(state_base));
    }

    let current_session_id = std::env::var("CSA_SESSION_ID").ok();
    let mut runtime_reap_enabled = false;
    let mut runtime_reap_stats = super::RuntimeReapStats::default();

    let now = chrono::Utc::now();
    let mut total_stale_locks = 0u64;
    let mut total_empty_sessions = 0u64;
    let mut total_orphan_dirs = 0u64;
    let mut total_expired_sessions = 0u64;
    let mut total_sessions_retired = 0u64;
    let mut total_transcripts_removed = 0u64;
    let mut total_transcript_bytes_reclaimed = 0u64;
    let mut projects_failed = 0u64;

    if dry_run {
        eprintln!("[dry-run] Global GC — no changes will be made.");
    }

    for session_root in &project_roots {
        // Use readonly variant in dry-run to avoid corrupt-state recovery writes
        let sessions = match if dry_run {
            list_sessions_from_root_readonly(session_root)
        } else {
            list_sessions_from_root(session_root)
        } {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    path = %session_root.display(),
                    error = %e,
                    "Failed to list sessions for project root (skipping)"
                );
                projects_failed += 1;
                continue;
            }
        };

        let mut project_removed = 0usize;
        for session in &sessions {
            let session_dir = session_root.join("sessions").join(&session.meta_session_id);
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
                                "[dry-run] Would remove stale lock (PID {}): {}",
                                pid,
                                path.display()
                            );
                            total_stale_locks += 1;
                        } else if fs::remove_file(&path).is_ok() {
                            info!("Removed stale lock (PID {}): {}", pid, path.display());
                            total_stale_locks += 1;
                        }
                    }
                }
            }

            if session.tools.is_empty() {
                if dry_run {
                    eprintln!(
                        "[dry-run] Would remove empty session: {} (in {})",
                        session.meta_session_id,
                        session_root.display()
                    );
                    total_empty_sessions += 1;
                    project_removed += 1;
                } else if csa_session::delete_session_from_root(
                    session_root,
                    &session.meta_session_id,
                )
                .is_ok()
                {
                    total_empty_sessions += 1;
                    project_removed += 1;
                }
                continue;
            }

            // Retire stale Active/Available sessions (>7 days since last access)
            let age = now.signed_duration_since(session.last_accessed);
            if let Some(retirement) =
                stale_session_retirement_candidate(session, now, RETIRE_AFTER_DAYS)
            {
                if dry_run {
                    eprintln!(
                        "[dry-run] Would retire stale session: {} (phase={}, {} days old, in {})",
                        session.meta_session_id,
                        session.phase,
                        retirement.age_days,
                        session_root.display()
                    );
                    total_sessions_retired += 1;
                } else {
                    let mut updated = session.clone();
                    updated.phase = retirement.phase;
                    match save_session_in(session_root, &updated) {
                        Ok(_) => {
                            info!(
                                session = %session.meta_session_id,
                                age_days = retirement.age_days,
                                root = %session_root.display(),
                                "Retired stale session"
                            );
                            total_sessions_retired += 1;
                        }
                        Err(e) => {
                            warn!(
                                session = %session.meta_session_id,
                                error = %e,
                                root = %session_root.display(),
                                "Failed to persist retirement"
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
                        "[dry-run] Would remove expired session: {} ({} days old, in {})",
                        session.meta_session_id,
                        age.num_days(),
                        session_root.display()
                    );
                    total_expired_sessions += 1;
                    project_removed += 1;
                } else if csa_session::delete_session_from_root(
                    session_root,
                    &session.meta_session_id,
                )
                .is_ok()
                {
                    info!(
                        "Removed expired session: {} (in {})",
                        session.meta_session_id,
                        session_root.display()
                    );
                    total_expired_sessions += 1;
                    project_removed += 1;
                }
            }
        }

        let sessions_dir = session_root.join("sessions");
        let sessions_is_real_dir = sessions_dir.is_dir()
            && !fs::symlink_metadata(&sessions_dir)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(true);
        if sessions_is_real_dir && let Ok(entries) = fs::read_dir(&sessions_dir) {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|ft| ft.is_dir()) && is_orphan_session_dir(&entry) {
                    let session_dir = entry.path();
                    if dry_run {
                        eprintln!(
                            "[dry-run] Would remove orphan directory: {}",
                            session_dir.display()
                        );
                        total_orphan_dirs += 1;
                    } else if fs::remove_dir_all(&session_dir).is_ok() {
                        info!("Removed orphan directory: {}", session_dir.display());
                        total_orphan_dirs += 1;
                    }
                }
            }
        }

        let rotation_path = session_root.join("rotation.toml");
        if rotation_path.exists() {
            let no_sessions_remain = if dry_run {
                project_removed >= sessions.len()
            } else {
                csa_session::list_sessions_from_root(session_root)
                    .map(|s| s.is_empty())
                    .unwrap_or(false)
            };
            if no_sessions_remain {
                if dry_run {
                    eprintln!("[dry-run] Would remove rotation state: {rotation_path:?}");
                } else if fs::remove_file(&rotation_path).is_ok() {
                    info!(
                        "Removed rotation state (no sessions remain): {:?}",
                        rotation_path
                    );
                }
            }
        }

        let project_gc_config = load_gc_config_for_sessions(session_root, &sessions);
        if let Some(days) =
            runtime_reap_max_age_days(reap_runtime, max_age_days, project_gc_config)?
        {
            runtime_reap_enabled = true;
            let sessions_for_reap = if dry_run {
                sessions_with_dry_run_retirements(&sessions, now, RETIRE_AFTER_DAYS)
            } else {
                match list_sessions_from_root(session_root) {
                    Ok(sessions) => sessions,
                    Err(err) => {
                        warn!(
                            path = %session_root.display(),
                            error = %err,
                            "Failed to refresh sessions before runtime reap; skipping project"
                        );
                        projects_failed += 1;
                        continue;
                    }
                }
            };
            match reap_runtime_payloads_in_root(
                session_root,
                &sessions_for_reap,
                dry_run,
                days,
                current_session_id.as_deref(),
            ) {
                Ok(stats) => merge_runtime_reap_stats(&mut runtime_reap_stats, stats),
                Err(err) => {
                    warn!(
                        path = %session_root.display(),
                        error = %err,
                        "Failed to reap runtime payloads for project; skipping project"
                    );
                    projects_failed += 1;
                }
            }
        }
        let transcript_stats =
            super::cleanup_project_transcripts(session_root, project_gc_config, dry_run);
        total_transcripts_removed =
            total_transcripts_removed.saturating_add(transcript_stats.files_removed);
        total_transcript_bytes_reclaimed =
            total_transcript_bytes_reclaimed.saturating_add(transcript_stats.bytes_reclaimed);
    }

    let runtime_reap_stats = runtime_reap_enabled.then_some(runtime_reap_stats);

    let mut orphan_scopes_cleaned = 0u64;
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

    let mut stale_slots_cleaned = 0u64;
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
                    let _ = fs::remove_dir(&tool_dir);
                }
            }
        }
    }

    match format {
        OutputFormat::Json => {
            let mut summary = serde_json::json!({
                "dry_run": dry_run,
                "global": true,
                "projects_scanned": project_roots.len(),
                "projects_failed": projects_failed,
                "stale_locks_removed": total_stale_locks,
                "empty_sessions_removed": total_empty_sessions,
                "orphan_dirs_removed": total_orphan_dirs,
                "sessions_retired": total_sessions_retired,
                "transcripts_removed": total_transcripts_removed,
                "transcript_bytes_reclaimed": total_transcript_bytes_reclaimed,
                "stale_slots_cleaned": stale_slots_cleaned,
                "orphan_scopes_cleaned": orphan_scopes_cleaned,
            });
            if !reap_runtime && max_age_days.is_some() {
                summary["expired_sessions_removed"] = serde_json::json!(total_expired_sessions);
            }
            if let Some(runtime_reap_stats) = runtime_reap_stats.as_ref() {
                summary["runtime_reap"] = serde_json::to_value(runtime_reap_stats)?;
            }
            println!("{}", serde_json::to_string_pretty(&summary)?);
        }
        OutputFormat::Text => {
            let prefix = if dry_run { "[dry-run] " } else { "" };
            eprintln!(
                "{}Global garbage collection {}:",
                prefix,
                if dry_run { "preview" } else { "complete" }
            );
            eprintln!("{}  Projects scanned: {}", prefix, project_roots.len());
            if projects_failed > 0 {
                eprintln!("{prefix}  Projects failed: {projects_failed}");
            }
            eprintln!("{prefix}  Stale locks removed: {total_stale_locks}");
            eprintln!("{prefix}  Empty sessions removed: {total_empty_sessions}");
            if total_sessions_retired > 0 {
                eprintln!("{prefix}  Sessions retired: {total_sessions_retired}");
            }
            if !reap_runtime && max_age_days.is_some() {
                eprintln!("{prefix}  Expired sessions removed: {total_expired_sessions}");
            }
            if let Some(runtime_reap_stats) = runtime_reap_stats.as_ref() {
                print_runtime_reap_summary(prefix, runtime_reap_stats);
            }
            eprintln!("{prefix}  Orphan directories removed: {total_orphan_dirs}");
            eprintln!(
                "{prefix}  Transcript files removed: {total_transcripts_removed} ({total_transcript_bytes_reclaimed} bytes)"
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

pub(crate) fn invalidate_state_dir_size_cache() {
    let state_roots = csa_config::paths::state_dir_all_roots();
    if state_roots.is_empty() {
        return;
    }

    // GC is the remediation path advertised by state-dir preflight; clear the cached aggregate.
    for state_dir in state_roots {
        let cache_path = state_dir.join(STATE_DIR_SIZE_CACHE_FILENAME);
        match fs::remove_file(&cache_path) {
            Ok(()) => {
                info!(path = %cache_path.display(), "Invalidated state directory size cache")
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => warn!(
                path = %cache_path.display(),
                error = %err,
                "Failed to invalidate state directory size cache"
            ),
        }
    }
}

/// Discover project roots (dirs with `sessions/` containing ULID dirs with `state.toml`).
pub(super) fn discover_project_roots(state_base: &Path) -> Vec<PathBuf> {
    let canonical_base = match state_base.canonicalize() {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    let mut roots = Vec::new();
    discover_roots_recursive(&canonical_base, &canonical_base, true, &mut roots);
    roots
}

/// Top-level CSA internal directories (not project paths) under the state base.
const TOP_LEVEL_SKIP: &[&str] = &["slots", "todos"];

fn discover_roots_recursive(
    dir: &Path,
    canonical_base: &Path,
    is_top_level: bool,
    roots: &mut Vec<PathBuf>,
) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        // Skip symlinks to prevent traversal outside state tree; file_type() does not follow them.
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if ft.is_symlink() || !ft.is_dir() {
            continue;
        }
        let path = entry.path();
        // Canonical path check: ensure we stay within state base
        let canonical = match path.canonicalize() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if !canonical.starts_with(canonical_base) {
            continue;
        }
        // Only skip known directories at the top level of the state base
        if is_top_level {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if TOP_LEVEL_SKIP.contains(&name_str.as_ref()) {
                continue;
            }
        }
        let sessions_path = path.join("sessions");
        // Accept sessions/ only if real dir (not symlink) with ULID entries or rotation state.
        let has_sessions = sessions_path.is_dir()
            && !fs::symlink_metadata(&sessions_path)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(true)
            && (has_confirmed_sessions(&sessions_path) || path.join("rotation.toml").exists());
        if has_sessions {
            roots.push(path.clone());
        }
        // Skip confirmed session containers (ULID subdirs with state.toml).
        // Path-segment "sessions" dirs with ULID-like children but no state.toml are traversed.
        let name = entry.file_name();
        if name.to_string_lossy() == "sessions" && has_confirmed_sessions(&path) {
            continue;
        }
        // Recurse to find nested sub-project roots (parent and child can coexist).
        discover_roots_recursive(&path, canonical_base, false, roots);
    }
}
