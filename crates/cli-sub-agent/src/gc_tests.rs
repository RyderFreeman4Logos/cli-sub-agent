use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_core::types::OutputFormat;
use std::os::unix::fs as unix_fs;
use tempfile::tempdir;

#[cfg(target_os = "linux")]
fn read_process_start_time_ticks(pid: u32) -> u64 {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let after_comm = stat.rsplit_once(") ").unwrap().1;
    after_comm
        .split_whitespace()
        .nth(19)
        .unwrap()
        .parse()
        .unwrap()
}

#[cfg(target_os = "linux")]
fn daemon_pid_record(pid: u32) -> String {
    format!("{pid} {}\n", read_process_start_time_ticks(pid))
}

/// Create a minimal project root with a session dir containing `state.toml`.
fn make_project_root(base: &std::path::Path, segments: &[&str]) {
    let mut path = base.to_path_buf();
    for s in segments {
        path = path.join(s);
    }
    let session_dir = path.join("sessions").join("01234567890123456789ABCDEF");
    fs::create_dir_all(&session_dir).unwrap();
    fs::write(session_dir.join("state.toml"), "").unwrap();
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
    let ext_session = external.path().join("sessions").join(ulid);
    fs::create_dir_all(&ext_session).unwrap();
    fs::write(ext_session.join("state.toml"), "").unwrap();
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
    let tmp_session = tmp.path().join("tmp").join("sessions").join(ulid);
    fs::create_dir_all(&tmp_session).unwrap();
    fs::write(tmp_session.join("state.toml"), "").unwrap();
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
fn test_extract_slot_lock_info_null_session_id() {
    let json = r#"{"pid":12345,"tool_name":"claude-code","slot_index":0,"acquired_at":"2024-01-01T00:00:00Z","session_id":null}"#;
    let (pid, session_id_is_null, acquired_at) = extract_slot_lock_info(json).unwrap();
    assert_eq!(pid, 12345);
    assert!(session_id_is_null);
    assert!(acquired_at.is_some());
}

#[test]
fn test_extract_slot_lock_info_with_session_id() {
    let json = r#"{"pid":12345,"tool_name":"claude-code","slot_index":0,"acquired_at":"2024-01-01T00:00:00Z","session_id":"01ABC123DEF456GHJ789KLMNOP"}"#;
    let (pid, session_id_is_null, _) = extract_slot_lock_info(json).unwrap();
    assert_eq!(pid, 12345);
    assert!(!session_id_is_null);
}

#[test]
fn test_extract_slot_lock_info_missing_session_id_treated_as_null() {
    // Absent session_id field is treated the same as JSON null.
    let json =
        r#"{"pid":99,"tool_name":"codex","slot_index":1,"acquired_at":"2024-01-01T00:00:00Z"}"#;
    let (pid, session_id_is_null, _) = extract_slot_lock_info(json).unwrap();
    assert_eq!(pid, 99);
    assert!(session_id_is_null);
}

#[test]
fn test_extract_slot_lock_info_missing_pid_returns_none() {
    let json = r#"{"tool_name":"codex","slot_index":0,"acquired_at":"2024-01-01T00:00:00Z","session_id":null}"#;
    assert!(extract_slot_lock_info(json).is_none());
}

#[test]
fn test_extract_slot_lock_info_invalid_json_returns_none() {
    assert!(extract_slot_lock_info("not json").is_none());
    assert!(extract_slot_lock_info("").is_none());
}

#[test]
fn test_extract_slot_lock_info_missing_acquired_at_is_ok() {
    // acquired_at absence is tolerated — returns None for the timestamp.
    let json = r#"{"pid":7,"tool_name":"opencode","slot_index":2,"session_id":null}"#;
    let (pid, session_id_is_null, acquired_at) = extract_slot_lock_info(json).unwrap();
    assert_eq!(pid, 7);
    assert!(session_id_is_null);
    assert!(acquired_at.is_none());
}

#[test]
fn test_orphan_slot_grace_period_within_window() {
    // A slot acquired 5 seconds ago is inside the 30s grace window and should be skipped.
    let now = chrono::Utc::now();
    let recent = (now - chrono::Duration::seconds(5)).to_rfc3339();
    let json = format!(
        r#"{{"pid":{},"tool_name":"claude-code","slot_index":0,"acquired_at":"{}","session_id":null}}"#,
        std::process::id(),
        recent
    );
    let (_, session_id_is_null, acquired_at) = extract_slot_lock_info(&json).unwrap();
    assert!(session_id_is_null);
    let age_secs = now
        .signed_duration_since(acquired_at.unwrap())
        .num_seconds();
    assert!(
        age_secs < ORPHAN_SLOT_GRACE_SECS,
        "slot acquired {age_secs}s ago is within {ORPHAN_SLOT_GRACE_SECS}s grace window"
    );
}

#[test]
fn test_orphan_slot_grace_period_outside_window() {
    // A slot acquired 60 seconds ago is outside the grace window and eligible for eviction.
    let now = chrono::Utc::now();
    let old = (now - chrono::Duration::seconds(60)).to_rfc3339();
    let json = format!(
        r#"{{"pid":{},"tool_name":"claude-code","slot_index":0,"acquired_at":"{}","session_id":null}}"#,
        std::process::id(),
        old
    );
    let (_, session_id_is_null, acquired_at) = extract_slot_lock_info(&json).unwrap();
    assert!(session_id_is_null);
    let age_secs = now
        .signed_duration_since(acquired_at.unwrap())
        .num_seconds();
    assert!(
        age_secs >= ORPHAN_SLOT_GRACE_SECS,
        "slot acquired {age_secs}s ago is outside {ORPHAN_SLOT_GRACE_SECS}s grace window"
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
    assert!(is_process_alive(std::process::id()));
}

#[test]
fn test_is_process_alive_dead() {
    assert!(!is_process_alive(4_000_000));
}

#[test]
fn test_orphan_cleanup_preserves_git_dir() {
    let tmp = tempdir().unwrap();
    let sessions = tmp.path().join("sessions");
    fs::create_dir_all(&sessions).unwrap();
    fs::create_dir_all(sessions.join(".git")).unwrap();
    // 27-char name (not valid ULID length) — never detected as orphan
    let valid = sessions.join("01AAAA0SESSI0N0000000000000");
    fs::create_dir_all(&valid).unwrap();
    fs::write(valid.join("state.toml"), "").unwrap();
    // Valid ULID (26 chars, no I/L/O/U) without state.toml — orphan
    let orphan_name = "01AAAA0000000000000000000B";
    fs::create_dir_all(sessions.join(orphan_name)).unwrap();
    let entries: Vec<_> = fs::read_dir(&sessions).unwrap().flatten().collect();
    let orphans: Vec<_> = entries
        .iter()
        .filter(|e| e.file_type().is_ok_and(|ft| ft.is_dir()) && is_orphan_session_dir(e))
        .collect();
    assert_eq!(orphans.len(), 1);
    assert_eq!(orphans[0].file_name().to_string_lossy(), orphan_name);
}

#[test]
fn test_orphan_check_skips_path_segments_and_non_ulid() {
    let tmp = tempdir().unwrap();
    let sessions = tmp.path().join("sessions");
    fs::create_dir_all(&sessions).unwrap();
    // Path segment (has sessions/ subdir) — not orphan regardless of valid ULID name
    fs::create_dir_all(sessions.join("01PATHSEG0000000000NESTED0").join("sessions")).unwrap();
    // Short name — not orphan (not 26 chars)
    fs::create_dir_all(sessions.join("short")).unwrap();
    // Valid ULID dir without state.toml or sessions/ = actual orphan
    let orphan_name = "01BBBB0000000000000000000C";
    fs::create_dir_all(sessions.join(orphan_name)).unwrap();
    let entries: Vec<_> = fs::read_dir(&sessions).unwrap().flatten().collect();
    let orphans: Vec<_> = entries
        .iter()
        .filter(|e| e.file_type().is_ok_and(|ft| ft.is_dir()) && is_orphan_session_dir(e))
        .collect();
    assert_eq!(
        orphans.len(),
        1,
        "Only valid-ULID dirs without state.toml are orphans"
    );
    assert_eq!(orphans[0].file_name().to_string_lossy(), orphan_name);
}

#[cfg(target_os = "linux")]
#[test]
fn test_orphan_cleanup_preserves_dir_with_live_daemon_pid() {
    let tmp = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project_root = tmp.path().join("project");
    fs::create_dir_all(&project_root).unwrap();
    let session_root = csa_session::get_session_root(&project_root).unwrap();
    let orphan_dir = session_root
        .join("sessions")
        .join("01CCCC0000000000000000000D");
    fs::create_dir_all(&orphan_dir).unwrap();
    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap();
    fs::write(orphan_dir.join("daemon.pid"), daemon_pid_record(child.id())).unwrap();
    assert!(csa_process::ToolLiveness::daemon_pid_is_alive(&orphan_dir));

    handle_gc(
        false,
        None,
        false,
        OutputFormat::Text,
        None,
        Some(project_root.to_str().unwrap()),
    )
    .unwrap();

    child.kill().ok();
    child.wait().ok();

    assert!(
        orphan_dir.exists(),
        "orphan-looking dir with live daemon.pid must be preserved"
    );
}

#[cfg(unix)]
#[test]
fn test_orphan_cleanup_preserves_dir_with_live_lock() {
    let tmp = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project_root = tmp.path().join("project");
    fs::create_dir_all(&project_root).unwrap();
    let session_root = csa_session::get_session_root(&project_root).unwrap();
    let orphan_dir = session_root
        .join("sessions")
        .join("01DDDD0000000000000000000E");
    fs::create_dir_all(&orphan_dir).unwrap();
    let _lock = csa_lock::acquire_lock(&orphan_dir, "codex", "orphan cleanup test").unwrap();
    assert!(csa_process::ToolLiveness::has_live_process(&orphan_dir));

    handle_gc(
        false,
        None,
        false,
        OutputFormat::Text,
        None,
        Some(project_root.to_str().unwrap()),
    )
    .unwrap();

    assert!(
        orphan_dir.exists(),
        "orphan-looking dir with live lock must be preserved"
    );
}

#[test]
fn test_discover_skips_symlinked_sessions_dir() {
    let tmp = tempdir().unwrap();
    let external = tempdir().unwrap();
    let ulid = "01234567890123456789ABCDEF";
    let ext_dir = external.path().join(ulid);
    fs::create_dir_all(&ext_dir).unwrap();
    fs::write(ext_dir.join("state.toml"), "").unwrap();
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

#[test]
fn test_discover_traverses_sessions_ulid_path_segment() {
    // Regression: sessions/<ULID>/ as a path segment must not block recursion
    let tmp = tempdir().unwrap();
    let ulid_segment = "01234567890123456789ABCDEF";
    // Project root is at sessions/<ULID>/project-a — "sessions" is a path segment
    make_project_root(
        tmp.path(),
        &["home", "user", "sessions", ulid_segment, "project-a"],
    );
    let roots = discover_project_roots(tmp.path());
    assert_eq!(
        roots.len(),
        1,
        "Must find root through sessions/<ULID> path segment"
    );
}

#[test]
fn test_discover_skips_orphan_only_without_confirmation() {
    // Orphan-only roots (ULID dir without state.toml or rotation.toml) are
    // indistinguishable from path segments, so global GC does NOT discover them.
    // Project-level GC handles these via direct session listing.
    let tmp = tempdir().unwrap();
    let ulid = "01234567890123456789ABCDEF";
    // sessions/<ulid> WITHOUT state.toml — ambiguous, not discovered by global GC
    fs::create_dir_all(tmp.path().join("orphan_proj").join("sessions").join(ulid)).unwrap();
    // sessions/<ulid> WITH state.toml — confirmed active root
    make_project_root(tmp.path(), &["active_proj"]);
    let roots = discover_project_roots(tmp.path());
    assert_eq!(
        roots.len(),
        1,
        "Only confirmed roots (with state.toml or rotation.toml) should be discovered"
    );
}

#[test]
fn test_discover_finds_rotation_only_roots() {
    // A project with empty sessions/ but rotation.toml should still be discovered
    let tmp = tempdir().unwrap();
    let proj = tmp.path().join("stale_proj");
    fs::create_dir_all(proj.join("sessions")).unwrap();
    fs::write(proj.join("rotation.toml"), "").unwrap();
    let roots = discover_project_roots(tmp.path());
    assert_eq!(
        roots.len(),
        1,
        "Root with empty sessions/ + rotation.toml should be discovered"
    );
}

#[test]
fn test_gc_global_invalidates_state_dir_size_cache() {
    let tmp = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let canonical = csa_config::paths::state_dir_write().expect("canonical state dir");
    let legacy = csa_config::paths::legacy_state_dir().expect("legacy state dir");

    for state_dir in [&canonical, &legacy] {
        fs::create_dir_all(state_dir).unwrap();
        let cache_path = state_dir.join(STATE_DIR_SIZE_CACHE_FILENAME);
        fs::write(
            &cache_path,
            r#"
size_bytes = 999999999
scanned_at = 1
"#,
        )
        .unwrap();
        assert!(
            cache_path.exists(),
            "test precondition: cache file must exist at {}",
            cache_path.display()
        );
    }

    handle_gc_global(false, None, false, OutputFormat::Text, None)
        .expect("global gc should succeed");

    for state_dir in [&canonical, &legacy] {
        let cache_path = state_dir.join(STATE_DIR_SIZE_CACHE_FILENAME);
        assert!(
            !cache_path.exists(),
            "gc must invalidate the cached state-dir size reading at {}",
            cache_path.display()
        );
    }
}

#[test]
fn test_gc_global_dry_run_preserves_state_dir_size_cache() {
    let tmp = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let canonical = csa_config::paths::state_dir_write().expect("canonical state dir");
    let legacy = csa_config::paths::legacy_state_dir().expect("legacy state dir");

    for state_dir in [&canonical, &legacy] {
        fs::create_dir_all(state_dir).unwrap();
        let cache_path = state_dir.join(STATE_DIR_SIZE_CACHE_FILENAME);
        fs::write(
            &cache_path,
            r#"
size_bytes = 999999999
scanned_at = 1
"#,
        )
        .unwrap();
        assert!(
            cache_path.exists(),
            "test precondition: cache file must exist at {}",
            cache_path.display()
        );
    }

    handle_gc_global(true, None, false, OutputFormat::Text, None)
        .expect("global dry-run gc should succeed");

    for state_dir in [&canonical, &legacy] {
        let cache_path = state_dir.join(STATE_DIR_SIZE_CACHE_FILENAME);
        assert!(
            cache_path.exists(),
            "dry-run gc must not invalidate the cached state-dir size reading at {}",
            cache_path.display()
        );
    }
}

#[test]
fn test_gc_global_preserves_active_empty_tools_session() {
    let tmp = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project_root = tmp.path().join("project");
    fs::create_dir_all(&project_root).unwrap();
    let mut session =
        csa_session::create_session(&project_root, Some("global active empty"), None, None)
            .unwrap();
    session.phase = csa_session::SessionPhase::Active;
    session.tools.clear();
    session.last_accessed = chrono::Utc::now() - chrono::Duration::days(40);
    csa_session::save_session(&session).unwrap();
    let session_dir =
        csa_session::get_session_dir(&project_root, &session.meta_session_id).unwrap();

    handle_gc_global(false, Some(1), false, OutputFormat::Text, None)
        .expect("global gc should succeed");

    assert!(
        session_dir.join("state.toml").exists(),
        "global empty-session sweep must preserve Active sessions"
    );
}

// --- Retirement logic tests ---

/// Verify that the retirement guard accepts Active and Available phases.
#[test]
fn test_retirement_guard_active_and_available() {
    use csa_session::state::{PhaseEvent, SessionPhase};

    let active = SessionPhase::Active;
    assert!(
        active.transition(&PhaseEvent::Retired).is_ok(),
        "Active sessions must be retirable"
    );

    let available = SessionPhase::Available;
    assert!(
        available.transition(&PhaseEvent::Retired).is_ok(),
        "Available sessions must be retirable"
    );
}

/// Verify that already-Retired sessions cannot be re-retired.
#[test]
fn test_retirement_guard_already_retired() {
    use csa_session::state::{PhaseEvent, SessionPhase};

    let retired = SessionPhase::Retired;
    assert!(
        retired.transition(&PhaseEvent::Retired).is_err(),
        "Already-retired sessions must not be re-retired"
    );
}

/// Verify that the retirement age threshold constant is 7 days.
#[test]
fn test_retire_after_days_threshold() {
    assert_eq!(RETIRE_AFTER_DAYS, 7, "Retirement threshold must be 7 days");
}

/// Verify that sessions younger than RETIRE_AFTER_DAYS are not eligible.
#[test]
fn test_retirement_age_check_young_session() {
    let now = chrono::Utc::now();
    // Session accessed 3 days ago — should NOT be retired
    let recent = now - chrono::Duration::days(3);
    let age = now.signed_duration_since(recent);
    assert!(
        age.num_days() <= RETIRE_AFTER_DAYS,
        "3-day-old session must not be retirement-eligible"
    );
}

/// Verify that sessions older than RETIRE_AFTER_DAYS are eligible.
#[test]
fn test_retirement_age_check_stale_session() {
    let now = chrono::Utc::now();
    // Session accessed 10 days ago — should be retired
    let stale = now - chrono::Duration::days(10);
    let age = now.signed_duration_since(stale);
    assert!(
        age.num_days() > RETIRE_AFTER_DAYS,
        "10-day-old session must be retirement-eligible"
    );
}

/// Verify that the combined guard (age + phase) correctly filters sessions.
#[test]
fn test_retirement_combined_guard() {
    use csa_session::state::{SessionPhase, ToolState};

    let now = chrono::Utc::now();
    let make_session = |phase, last_accessed| {
        let mut session = csa_session::MetaSessionState {
            phase,
            last_accessed,
            ..Default::default()
        };
        session.tools.insert(
            "codex".to_string(),
            ToolState {
                provider_session_id: None,
                last_action_summary: String::new(),
                last_exit_code: 0,
                updated_at: now,
                tool_version: None,
                token_usage: None,
            },
        );
        session
    };

    // Case 1: Old Active → eligible
    let stale_active = make_session(SessionPhase::Active, now - chrono::Duration::days(10));
    let candidate = stale_session_retirement_candidate(&stale_active, now, RETIRE_AFTER_DAYS)
        .expect("old active session should be retirement-eligible");
    assert_eq!(candidate.phase, SessionPhase::Retired);
    assert_eq!(candidate.age_days, 10);

    // Case 2: Young Active → not eligible (age guard fails)
    let recent_active = make_session(SessionPhase::Active, now - chrono::Duration::days(3));
    assert!(stale_session_retirement_candidate(&recent_active, now, RETIRE_AFTER_DAYS).is_none());

    // Case 3: Old Retired → not eligible (phase guard fails)
    let stale_retired = make_session(SessionPhase::Retired, now - chrono::Duration::days(10));
    assert!(stale_session_retirement_candidate(&stale_retired, now, RETIRE_AFTER_DAYS).is_none());
}
