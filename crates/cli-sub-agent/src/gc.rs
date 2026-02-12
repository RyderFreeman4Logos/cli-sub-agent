use anyhow::Result;
use std::fs;
use tracing::info;

use csa_config::GlobalConfig;
use csa_core::types::OutputFormat;
use csa_session::{delete_session, get_session_dir, list_sessions};

pub(crate) fn handle_gc(
    dry_run: bool,
    max_age_days: Option<u64>,
    format: OutputFormat,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(None)?;
    let sessions = list_sessions(&project_root, None)?;
    let now = chrono::Utc::now();

    let mut stale_locks_removed = 0;
    let mut empty_sessions_removed = 0;
    let mut orphan_dirs_removed = 0;
    let mut expired_sessions_removed = 0;

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
            } else if delete_session(&project_root, &session.meta_session_id).is_ok() {
                empty_sessions_removed += 1;
            }
        }

        if let Some(days) = max_age_days {
            let age = now.signed_duration_since(session.last_accessed);
            if age.num_days() > days as i64 {
                if dry_run {
                    eprintln!(
                        "[dry-run] Would remove expired session: {} (last accessed {} days ago)",
                        session.meta_session_id,
                        age.num_days()
                    );
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
        // If no sessions remain (all removed above), clean rotation state
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
                                                        path.file_name(), pid
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
    if project_roots.is_empty() {
        eprintln!(
            "No project session roots found under {}",
            state_base.display()
        );
        return Ok(());
    }

    let now = chrono::Utc::now();
    let mut total_stale_locks = 0u64;
    let mut total_empty_sessions = 0u64;
    let mut total_orphan_dirs = 0u64;
    let mut total_expired_sessions = 0u64;
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

            if let Some(days) = max_age_days {
                let age = now.signed_duration_since(session.last_accessed);
                if age.num_days() > days as i64 {
                    if dry_run {
                        eprintln!(
                            "[dry-run] Would remove expired session: {} ({} days old, in {})",
                            session.meta_session_id,
                            age.num_days(),
                            session_root.display()
                        );
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

        // Skip if sessions_dir is a symlink (safety: avoid traversal outside state base)
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
                // Use tracked counter: all sessions would be removed
                project_removed >= sessions.len()
            } else {
                csa_session::list_sessions_from_root(session_root)
                    .unwrap_or_default()
                    .is_empty()
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
                                                        path.file_name(), pid
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
            eprintln!("{}  Stale slots cleaned: {}", prefix, stale_slots_cleaned);
        }
    }

    Ok(())
}

/// Discover project roots (dirs with ULID-containing `sessions/`) under state base.
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
        // Accept sessions/ only if real dir (not symlink) with ULID-length entries.
        let has_sessions = sessions_path.is_dir()
            && !fs::symlink_metadata(&sessions_path)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(true)
            && looks_like_session_container(&sessions_path);
        if has_sessions {
            roots.push(path.clone());
        }
        // Skip real session containers; path-segment "sessions" dirs are traversed.
        let name = entry.file_name();
        if name.to_string_lossy() == "sessions" && looks_like_session_container(&path) {
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

/// Returns `true` for ULID-length non-hidden dirs in `sessions/` lacking `state.toml`.
fn is_orphan_session_dir(entry: &fs::DirEntry) -> bool {
    let name = entry.file_name();
    let name_str = name.to_string_lossy();
    if name_str.starts_with('.') {
        return false;
    }
    // Only ULID-length (26 char) dirs can be orphan sessions.
    if name_str.len() != 26 {
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

/// Check if a directory looks like a session container (has 26-char ULID entries).
fn looks_like_session_container(dir: &std::path::Path) -> bool {
    fs::read_dir(dir).is_ok_and(|rd| {
        rd.flatten()
            .any(|e| e.file_type().is_ok_and(|ft| ft.is_dir()) && e.file_name().len() == 26)
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
mod tests {
    use super::*;
    use std::os::unix::fs as unix_fs;
    use tempfile::tempdir;

    fn make_project_root(base: &std::path::Path, segments: &[&str]) {
        let mut path = base.to_path_buf();
        for s in segments {
            path = path.join(s);
        }
        fs::create_dir_all(path.join("sessions").join("01234567890123456789ABCDEF")).unwrap();
    }

    #[test]
    fn test_discover_finds_nested_project_roots() {
        let tmp = tempdir().unwrap();
        make_project_root(tmp.path(), &["home", "user", "project"]);
        make_project_root(tmp.path(), &["home", "user", "other"]);

        let roots = discover_project_roots(tmp.path());
        assert_eq!(roots.len(), 2);
    }

    #[test]
    fn test_discover_skips_symlinks() {
        let tmp = tempdir().unwrap();
        let external = tempdir().unwrap();
        let ulid = "01234567890123456789ABCDEF";
        fs::create_dir_all(external.path().join("sessions").join(ulid)).unwrap();
        unix_fs::symlink(external.path(), tmp.path().join("evil_link")).unwrap();
        let roots = discover_project_roots(tmp.path());
        assert!(roots.is_empty());
    }

    #[test]
    fn test_discover_skips_top_level_only() {
        let tmp = tempdir().unwrap();
        let ulid = "01234567890123456789ABCDEF";
        fs::create_dir_all(tmp.path().join("slots").join("sessions").join(ulid)).unwrap();
        fs::create_dir_all(tmp.path().join("todos").join("sessions").join(ulid)).unwrap();
        fs::create_dir_all(tmp.path().join("tmp").join("sessions").join(ulid)).unwrap();
        make_project_root(tmp.path(), &["home", "user", "tmp", "myproject"]);
        let roots = discover_project_roots(tmp.path());
        assert_eq!(roots.len(), 2);
    }

    #[test]
    fn test_discover_ignores_nested_sessions_in_artifacts() {
        let tmp = tempdir().unwrap();
        make_project_root(tmp.path(), &["home", "user", "proj"]);
        let nested = tmp
            .path()
            .join("home/user/proj/sessions/01ARZ3NDEK/output/cache/sessions");
        fs::create_dir_all(nested.join("random-dir")).unwrap();
        let roots = discover_project_roots(tmp.path());
        assert_eq!(roots.len(), 1, "Only the real project root should be found");
    }

    #[test]
    fn test_extract_pid_from_lock_valid() {
        assert_eq!(extract_pid_from_lock(r#"{"pid": 12345}"#), Some(12345));
    }

    #[test]
    fn test_extract_pid_from_lock_invalid() {
        assert_eq!(extract_pid_from_lock("not json"), None);
        assert_eq!(extract_pid_from_lock(r#"{"no_pid": 1}"#), None);
    }

    #[test]
    fn test_extract_pid_from_lock_overflow_rejected() {
        assert_eq!(extract_pid_from_lock(r#"{"pid": 4294967297}"#), None);
        assert_eq!(
            extract_pid_from_lock(r#"{"pid": 18446744073709551615}"#),
            None
        );
    }

    #[test]
    fn test_discover_finds_ancestor_and_descendant_roots() {
        let tmp = tempdir().unwrap();
        make_project_root(tmp.path(), &["home", "user"]);
        make_project_root(tmp.path(), &["home", "user", "subproject"]);

        let roots = discover_project_roots(tmp.path());
        assert_eq!(
            roots.len(),
            2,
            "Both ancestor and descendant roots must be discovered"
        );
    }

    #[test]
    fn test_is_process_alive_self() {
        // Current process should always be alive
        assert!(is_process_alive(std::process::id()));
    }

    #[test]
    fn test_is_process_alive_dead() {
        // PID 0 is kernel, likely not accessible; very high PID unlikely to exist
        assert!(!is_process_alive(4_000_000));
    }

    #[test]
    fn test_orphan_cleanup_preserves_git_dir() {
        let tmp = tempdir().unwrap();
        let sessions = tmp.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::create_dir_all(sessions.join(".git")).unwrap();
        let valid = sessions.join("01VALID0SESSION0ID000000000");
        fs::create_dir_all(&valid).unwrap();
        fs::write(valid.join("state.toml"), "").unwrap();
        // Orphan must be ULID-length (26 chars) to be detected
        fs::create_dir_all(sessions.join("01ORPHAN000000000NOSTATE00")).unwrap();
        let entries: Vec<_> = fs::read_dir(&sessions).unwrap().flatten().collect();
        let orphans: Vec<_> = entries
            .iter()
            .filter(|e| e.file_type().is_ok_and(|ft| ft.is_dir()) && is_orphan_session_dir(e))
            .collect();
        assert_eq!(orphans.len(), 1);
        assert_eq!(
            orphans[0].file_name().to_string_lossy(),
            "01ORPHAN000000000NOSTATE00"
        );
    }

    #[test]
    fn test_orphan_check_skips_path_segments_and_non_ulid() {
        let tmp = tempdir().unwrap();
        let sessions = tmp.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        // Path segment (has sessions/ subdir) — not orphan regardless of name length
        fs::create_dir_all(sessions.join("01PATHSEG0000000000NESTED0").join("sessions")).unwrap();
        // Short name — not orphan (not ULID-length)
        fs::create_dir_all(sessions.join("short")).unwrap();
        // ULID-length dir without state.toml or sessions/ = actual orphan
        fs::create_dir_all(sessions.join("01ORPHAN000000000REALONE00")).unwrap();
        let entries: Vec<_> = fs::read_dir(&sessions).unwrap().flatten().collect();
        let orphans: Vec<_> = entries
            .iter()
            .filter(|e| e.file_type().is_ok_and(|ft| ft.is_dir()) && is_orphan_session_dir(e))
            .collect();
        assert_eq!(
            orphans.len(),
            1,
            "Only ULID-length dirs without state.toml are orphans"
        );
        assert_eq!(
            orphans[0].file_name().to_string_lossy(),
            "01ORPHAN000000000REALONE00"
        );
    }

    #[test]
    fn test_discover_skips_symlinked_sessions_dir() {
        let tmp = tempdir().unwrap();
        let external = tempdir().unwrap();
        let ulid = "01234567890123456789ABCDEF";
        fs::create_dir_all(external.path().join(ulid)).unwrap();
        let dir = tmp.path().join("project");
        fs::create_dir_all(&dir).unwrap();
        unix_fs::symlink(external.path(), dir.join("sessions")).unwrap();
        let roots = discover_project_roots(tmp.path());
        assert!(
            roots.is_empty(),
            "Symlinked sessions/ must not be treated as root"
        );
    }

    #[test]
    fn test_discover_traverses_sessions_path_segment() {
        let tmp = tempdir().unwrap();
        // Project at /home/user/sessions/app — "sessions" is a path segment
        make_project_root(tmp.path(), &["home", "user", "sessions", "app"]);
        let roots = discover_project_roots(tmp.path());
        assert_eq!(
            roots.len(),
            1,
            "Must find root through sessions path segment"
        );
    }
}
