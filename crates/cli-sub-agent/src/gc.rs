use anyhow::Result;
use std::fs;
use tracing::{info, warn};

use csa_config::{GcConfig, GlobalConfig};
use csa_core::types::OutputFormat;
use csa_session::{
    PhaseEvent, delete_session, get_session_dir, get_session_root, list_sessions, save_session_in,
};

mod transcript;
use transcript::{cleanup_project_transcripts, load_gc_config_for_sessions};

/// Default age threshold (in days) for retiring stale Active sessions.
const RETIRE_AFTER_DAYS: i64 = 7;

pub(crate) fn handle_gc(
    dry_run: bool,
    max_age_days: Option<u64>,
    format: OutputFormat,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(None)?;
    let session_root = get_session_root(&project_root)?;
    let sessions = list_sessions(&project_root, None)?;
    let gc_config = GcConfig::load_for_project(&project_root)?;
    let now = chrono::Utc::now();

    let mut stale_locks_removed = 0;
    let mut empty_sessions_removed = 0;
    let mut orphan_dirs_removed = 0;
    let mut expired_sessions_removed = 0;
    let mut sessions_retired = 0u64;

    if dry_run {
        eprintln!("[dry-run] No changes will be made.");
    }

    for session in &sessions {
        let session_dir = get_session_dir(&project_root, &session.meta_session_id)?;
        let locks_dir = session_dir.join("locks");

        if locks_dir.exists() {
            if let Ok(entries) = fs::read_dir(&locks_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().is_some_and(|ext| ext == "lock") {
                        if let Ok(content) = fs::read_to_string(&path) {
                            if let Some(pid) = extract_pid_from_lock(&content) {
                                if !is_process_alive(pid) {
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

        if let Some(days) = max_age_days {
            if age.num_days() > days as i64 {
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
    }

    let session_root = csa_session::get_session_root(&project_root)?;
    let rotation_path = session_root.join("rotation.toml");
    if rotation_path.exists() {
        let remaining = list_sessions(&project_root, None)?;
        if remaining.is_empty() {
            if dry_run {
                eprintln!("[dry-run] Would remove rotation state: {:?}", rotation_path);
            } else if fs::remove_file(&rotation_path).is_ok() {
                info!("Removed rotation state (no sessions remain)");
            }
        }
    }

    let sessions_dir = session_root.join("sessions");

    if sessions_dir.exists() {
        if let Ok(entries) = fs::read_dir(&sessions_dir) {
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
    }

    let transcript_stats = cleanup_project_transcripts(&session_root, gc_config, dry_run);

    let mut stale_slots_cleaned = 0;
    if let Ok(slots_dir) = GlobalConfig::slots_dir() {
        if slots_dir.exists() {
            if let Ok(entries) = fs::read_dir(&slots_dir) {
                for entry in entries.flatten() {
                    if entry.file_type().is_ok_and(|ft| ft.is_dir()) {
                        let tool_dir = entry.path();
                        if let Ok(slot_entries) = fs::read_dir(&tool_dir) {
                            for slot_entry in slot_entries.flatten() {
                                let path = slot_entry.path();
                                if path.extension().is_some_and(|ext| ext == "lock") {
                                    if let Ok(content) = fs::read_to_string(&path) {
                                        if let Some(pid) = extract_pid_from_lock(&content) {
                                            if !is_process_alive(pid) {
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
                                }
                            }
                        }
                        if !dry_run {
                            let _ = fs::remove_dir(&tool_dir); // only succeeds if empty
                        }
                    }
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
            });
            if max_age_days.is_some() {
                summary["expired_sessions_removed"] = serde_json::json!(expired_sessions_removed);
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
            eprintln!("{}  Stale locks removed: {}", prefix, stale_locks_removed);
            eprintln!(
                "{}  Empty sessions removed: {}",
                prefix, empty_sessions_removed
            );
            if sessions_retired > 0 {
                eprintln!("{}  Sessions retired: {}", prefix, sessions_retired);
            }
            if max_age_days.is_some() {
                eprintln!(
                    "{}  Expired sessions removed: {}",
                    prefix, expired_sessions_removed
                );
            }
            eprintln!(
                "{}  Orphan directories removed: {}",
                prefix, orphan_dirs_removed
            );
            eprintln!(
                "{}  Transcript files removed: {} ({} bytes)",
                prefix, transcript_stats.files_removed, transcript_stats.bytes_reclaimed
            );
            eprintln!("{}  Stale slots cleaned: {}", prefix, stale_slots_cleaned);
        }
    }

    Ok(())
}

/// Global GC: scan all project session roots under `~/.local/state/csa/`.
///
/// Discovers project roots by recursively finding directories that contain
/// a `sessions/` subdirectory, then applies the same cleanup criteria as
/// per-project GC plus cross-project slot cleanup.
pub(crate) fn handle_gc_global(
    dry_run: bool,
    max_age_days: Option<u64>,
    format: OutputFormat,
) -> Result<()> {
    let state_base = GlobalConfig::state_base_dir()?;
    if !state_base.exists() {
        eprintln!("No CSA state directory found at {}", state_base.display());
        return Ok(());
    }

    let project_roots = discover_project_roots(&state_base);

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
            csa_session::list_sessions_from_root_readonly(session_root)
        } else {
            csa_session::list_sessions_from_root(session_root)
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

        let mut project_removed = 0usize; // track per-project removals for rotation preview
        for session in &sessions {
            let session_dir = session_root.join("sessions").join(&session.meta_session_id);
            let locks_dir = session_dir.join("locks");

            if locks_dir.exists() {
                if let Ok(entries) = fs::read_dir(&locks_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().is_some_and(|ext| ext == "lock") {
                            if let Ok(content) = fs::read_to_string(&path) {
                                if let Some(pid) = extract_pid_from_lock(&content) {
                                    if !is_process_alive(pid) {
                                        if dry_run {
                                            eprintln!(
                                                "[dry-run] Would remove stale lock (PID {}): {}",
                                                pid,
                                                path.display()
                                            );
                                            total_stale_locks += 1;
                                        } else if fs::remove_file(&path).is_ok() {
                                            info!(
                                                "Removed stale lock (PID {}): {}",
                                                pid,
                                                path.display()
                                            );
                                            total_stale_locks += 1;
                                        }
                                    }
                                }
                            }
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
            if age.num_days() > RETIRE_AFTER_DAYS
                && session.phase.transition(&PhaseEvent::Retired).is_ok()
            {
                if dry_run {
                    eprintln!(
                        "[dry-run] Would retire stale session: {} (phase={}, {} days old, in {})",
                        session.meta_session_id,
                        session.phase,
                        age.num_days(),
                        session_root.display()
                    );
                    total_sessions_retired += 1;
                } else {
                    let mut updated = session.clone();
                    match updated.phase.transition(&PhaseEvent::Retired) {
                        Ok(new_phase) => {
                            updated.phase = new_phase;
                            match save_session_in(session_root, &updated) {
                                Ok(_) => {
                                    info!(
                                        session = %session.meta_session_id,
                                        age_days = age.num_days(),
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

            if let Some(days) = max_age_days {
                if age.num_days() > days as i64 {
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
        }

        let sessions_dir = session_root.join("sessions");
        let sessions_is_real_dir = sessions_dir.is_dir()
            && !fs::symlink_metadata(&sessions_dir)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(true);
        if sessions_is_real_dir {
            if let Ok(entries) = fs::read_dir(&sessions_dir) {
                for entry in entries.flatten() {
                    if entry.file_type().is_ok_and(|ft| ft.is_dir())
                        && is_orphan_session_dir(&entry)
                    {
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
        }

        let rotation_path = session_root.join("rotation.toml");
        if rotation_path.exists() {
            let no_sessions_remain = if dry_run {
                project_removed >= sessions.len()
            } else {
                csa_session::list_sessions_from_root(session_root)
                    .map(|s| s.is_empty())
                    .unwrap_or(false) // treat error as "sessions might still exist"
            };
            if no_sessions_remain {
                if dry_run {
                    eprintln!("[dry-run] Would remove rotation state: {:?}", rotation_path);
                } else if fs::remove_file(&rotation_path).is_ok() {
                    info!(
                        "Removed rotation state (no sessions remain): {:?}",
                        rotation_path
                    );
                }
            }
        }

        let project_gc_config = load_gc_config_for_sessions(session_root, &sessions);
        let transcript_stats =
            cleanup_project_transcripts(session_root, project_gc_config, dry_run);
        total_transcripts_removed =
            total_transcripts_removed.saturating_add(transcript_stats.files_removed);
        total_transcript_bytes_reclaimed =
            total_transcript_bytes_reclaimed.saturating_add(transcript_stats.bytes_reclaimed);
    }

    let mut stale_slots_cleaned = 0u64;
    if let Ok(slots_dir) = GlobalConfig::slots_dir() {
        if slots_dir.exists() {
            if let Ok(entries) = fs::read_dir(&slots_dir) {
                for entry in entries.flatten() {
                    if entry.file_type().is_ok_and(|ft| ft.is_dir()) {
                        let tool_dir = entry.path();
                        if let Ok(slot_entries) = fs::read_dir(&tool_dir) {
                            for slot_entry in slot_entries.flatten() {
                                let path = slot_entry.path();
                                if path.extension().is_some_and(|ext| ext == "lock") {
                                    if let Ok(content) = fs::read_to_string(&path) {
                                        if let Some(pid) = extract_pid_from_lock(&content) {
                                            if !is_process_alive(pid) {
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
                                }
                            }
                        }
                        if !dry_run {
                            let _ = fs::remove_dir(&tool_dir);
                        }
                    }
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
            });
            if max_age_days.is_some() {
                summary["expired_sessions_removed"] = serde_json::json!(total_expired_sessions);
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
                eprintln!("{}  Projects failed: {}", prefix, projects_failed);
            }
            eprintln!("{}  Stale locks removed: {}", prefix, total_stale_locks);
            eprintln!(
                "{}  Empty sessions removed: {}",
                prefix, total_empty_sessions
            );
            if total_sessions_retired > 0 {
                eprintln!("{}  Sessions retired: {}", prefix, total_sessions_retired);
            }
            if max_age_days.is_some() {
                eprintln!(
                    "{}  Expired sessions removed: {}",
                    prefix, total_expired_sessions
                );
            }
            eprintln!(
                "{}  Orphan directories removed: {}",
                prefix, total_orphan_dirs
            );
            eprintln!(
                "{}  Transcript files removed: {} ({} bytes)",
                prefix, total_transcripts_removed, total_transcript_bytes_reclaimed
            );
            eprintln!("{}  Stale slots cleaned: {}", prefix, stale_slots_cleaned);
        }
    }

    Ok(())
}

/// Discover project roots (dirs with `sessions/` containing ULID dirs with `state.toml`).
/// Skips symlinks, validates canonical paths, and skips `slots/`/`todos/` at top level.
fn discover_project_roots(state_base: &std::path::Path) -> Vec<std::path::PathBuf> {
    let canonical_base = match state_base.canonicalize() {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    let mut roots = Vec::new();
    discover_roots_recursive(&canonical_base, &canonical_base, true, &mut roots);
    roots
}

/// Top-level CSA internal directories (not project paths) under the state base.
/// Note: "tmp" is NOT skipped because `/tmp/...` project paths map to `<state>/csa/tmp/...`.
const TOP_LEVEL_SKIP: &[&str] = &["slots", "todos"];

fn discover_roots_recursive(
    dir: &std::path::Path,
    canonical_base: &std::path::Path,
    is_top_level: bool,
    roots: &mut Vec<std::path::PathBuf>,
) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        // Skip symlinks to prevent traversal outside state tree.
        // Use file_type() which does NOT follow symlinks (unlike metadata()).
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
fn has_confirmed_sessions(dir: &std::path::Path) -> bool {
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
