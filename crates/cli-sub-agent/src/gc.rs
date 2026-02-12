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

        // 1. Check for stale locks
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

        // 2. Check for empty sessions (no tools used)
        if session.tools.is_empty() {
            if dry_run {
                eprintln!(
                    "[dry-run] Would remove empty session: {}",
                    session.meta_session_id
                );
            } else {
                let _ = delete_session(&project_root, &session.meta_session_id);
            }
            empty_sessions_removed += 1;
        }

        // 3. Check for expired sessions (--max-age-days)
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
                }
                expired_sessions_removed += 1;
            }
        }
    }

    // Clean rotation.toml if all sessions are gone
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

    // Clean orphan directories (no state.toml, excluding hidden dirs like .git)
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
                    } else {
                        let _ = fs::remove_dir_all(&session_dir);
                        info!(
                            "Removed orphan directory without state.toml: {}",
                            session_dir.display()
                        );
                    }
                    orphan_dirs_removed += 1;
                }
            }
        }
    }

    // Clean stale slot files (global, not per-project)
    let mut stale_slots_cleaned = 0;
    if let Ok(slots_dir) = GlobalConfig::slots_dir() {
        if slots_dir.exists() {
            if let Ok(entries) = fs::read_dir(&slots_dir) {
                for entry in entries.flatten() {
                    if entry.file_type().is_ok_and(|ft| ft.is_dir()) {
                        let tool_dir = entry.path();
                        // Check each slot file for stale PIDs
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
                        // Remove empty tool directories
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
            Err(_) => continue,
        };

        let mut project_removed = 0usize; // track per-project removals for rotation preview
        for session in &sessions {
            let session_dir = session_root.join("sessions").join(&session.meta_session_id);
            let locks_dir = session_dir.join("locks");

            // Stale locks
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

            // Empty sessions (mutually exclusive with expired — skip expired if empty)
            if session.tools.is_empty() {
                if dry_run {
                    eprintln!(
                        "[dry-run] Would remove empty session: {} (in {})",
                        session.meta_session_id,
                        session_root.display()
                    );
                } else {
                    let _ = csa_session::delete_session_from_root(
                        session_root,
                        &session.meta_session_id,
                    );
                }
                total_empty_sessions += 1;
                project_removed += 1;
                continue;
            }

            // Expired sessions
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
                    }
                    total_expired_sessions += 1;
                    project_removed += 1;
                }
            }
        }

        // Orphan directories (excluding hidden dirs like .git)
        let sessions_dir = session_root.join("sessions");
        if sessions_dir.exists() {
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
                        } else {
                            let _ = fs::remove_dir_all(&session_dir);
                            info!("Removed orphan directory: {}", session_dir.display());
                        }
                        total_orphan_dirs += 1;
                    }
                }
            }
        }

        // Clean rotation.toml if project has no sessions remaining
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

    // Also clean stale slots (same as per-project GC)
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

/// Discover all project session roots under the CSA state base directory.
///
/// A project root is any directory that contains a `sessions/` subdirectory.
/// Walks the tree recursively with safety guards:
/// - Skips symlinks to prevent traversal outside the state tree
/// - Validates canonical paths stay within the state base
/// - Skips known top-level non-project directories (`slots/`, `tmp/`, `todos/`)
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
        let has_sessions = path.join("sessions").is_dir();
        if has_sessions {
            roots.push(path.clone());
        }
        // Skip directories named "sessions" to avoid recursing into session
        // artifact trees (which may contain nested sessions/ dirs that would
        // be falsely identified as project roots).
        let name = entry.file_name();
        if name.to_string_lossy() == "sessions" {
            continue;
        }
        // Recurse to find nested sub-project roots (parent and child can coexist).
        discover_roots_recursive(&path, canonical_base, false, roots);
    }
}

/// Extract PID from lock file JSON content.
///
/// Lock files contain JSON with a `pid` field (see `LockDiagnostic` and
/// `SlotDiagnostic` in `csa-lock`). Uses serde_json for robust parsing.
fn extract_pid_from_lock(json_content: &str) -> Option<u32> {
    let v: serde_json::Value = serde_json::from_str(json_content).ok()?;
    let n = v.get("pid")?.as_u64()?;
    u32::try_from(n).ok()
}

/// Check if a directory inside `sessions/` is an orphan that should be cleaned.
///
/// Returns `true` for non-hidden directories that lack `state.toml`.
/// Hidden directories (`.git`, etc.) are never considered orphans.
fn is_orphan_session_dir(entry: &fs::DirEntry) -> bool {
    let name = entry.file_name();
    if name.to_string_lossy().starts_with('.') {
        return false;
    }
    !entry.path().join("state.toml").exists()
}

/// Check if a process is alive (cross-platform Unix).
fn is_process_alive(pid: u32) -> bool {
    // kill(pid, 0) checks existence without sending a signal.
    // Returns 0 if the process exists (and we have permission),
    // or -1 with EPERM if it exists but we lack permission.
    // SAFETY: signal 0 is a null signal that performs error checking
    // but does not actually send a signal.
    let ret = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if ret == 0 {
        return true;
    }
    // EPERM means the process exists but we can't signal it.
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
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
        fs::create_dir_all(path.join("sessions")).unwrap();
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
        // Create a real project root inside the external dir
        fs::create_dir_all(external.path().join("sessions")).unwrap();
        // Create a symlink inside state base pointing to external
        unix_fs::symlink(external.path(), tmp.path().join("evil_link")).unwrap();

        let roots = discover_project_roots(tmp.path());
        // The symlinked directory must NOT be discovered
        assert!(roots.is_empty());
    }

    #[test]
    fn test_discover_skips_top_level_only() {
        let tmp = tempdir().unwrap();
        // "slots" at top level should be skipped (CSA internal)
        fs::create_dir_all(tmp.path().join("slots").join("sessions")).unwrap();
        // "todos" at top level should be skipped (CSA internal)
        fs::create_dir_all(tmp.path().join("todos").join("sessions")).unwrap();
        // "tmp" at top level should NOT be skipped (legitimate /tmp projects)
        fs::create_dir_all(tmp.path().join("tmp").join("sessions")).unwrap();
        // "tmp" nested inside a project path should NOT be skipped
        make_project_root(tmp.path(), &["home", "user", "tmp", "myproject"]);

        let roots = discover_project_roots(tmp.path());
        // Both tmp (top-level) and nested myproject found; only slots/todos skipped
        assert_eq!(roots.len(), 2);
    }

    #[test]
    fn test_discover_ignores_nested_sessions_in_artifacts() {
        let tmp = tempdir().unwrap();
        // Real project root
        make_project_root(tmp.path(), &["home", "user", "proj"]);
        // Nested sessions/ inside a session's output — must NOT be a root
        let nested = tmp
            .path()
            .join("home/user/proj/sessions/01ARZ3NDEK/output/cache/sessions");
        fs::create_dir_all(nested.join("random-dir")).unwrap();

        let roots = discover_project_roots(tmp.path());
        assert_eq!(roots.len(), 1, "Only the real project root should be found");
        assert!(
            roots[0].to_string_lossy().contains("home/user/proj"),
            "Root should be the actual project, not nested artifact"
        );
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
        // PID > u32::MAX must be rejected, not silently truncated
        assert_eq!(
            extract_pid_from_lock(r#"{"pid": 4294967297}"#),
            None,
            "PID > u32::MAX should return None"
        );
        assert_eq!(
            extract_pid_from_lock(r#"{"pid": 18446744073709551615}"#),
            None,
            "u64::MAX PID should return None"
        );
    }

    #[test]
    fn test_discover_finds_ancestor_and_descendant_roots() {
        let tmp = tempdir().unwrap();
        // Parent is a root (has sessions/)
        make_project_root(tmp.path(), &["home", "user"]);
        // Child is also a root (nested inside parent)
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

        // .git should be preserved (hidden infrastructure)
        fs::create_dir_all(sessions.join(".git")).unwrap();
        // Valid session with state.toml should be preserved
        let valid = sessions.join("valid-session");
        fs::create_dir_all(&valid).unwrap();
        fs::write(valid.join("state.toml"), "").unwrap();
        // Orphan directory without state.toml should be detected
        fs::create_dir_all(sessions.join("orphan-no-state")).unwrap();

        let entries: Vec<_> = fs::read_dir(&sessions).unwrap().flatten().collect();
        let orphans: Vec<_> = entries
            .iter()
            .filter(|e| e.file_type().is_ok_and(|ft| ft.is_dir()) && is_orphan_session_dir(e))
            .collect();

        assert_eq!(orphans.len(), 1, "Only the orphan should be detected");
        assert_eq!(orphans[0].file_name().to_string_lossy(), "orphan-no-state");
    }
}
