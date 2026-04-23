use super::*;
use crate::session_cmds_result::{StructuredOutputOpts, handle_session_result};
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_core::types::OutputFormat;
use csa_session::{
    SessionArtifact, SessionPhase, SessionResult, create_session, get_session_root, list_sessions,
    save_result, save_session,
};
use std::os::unix::fs as unix_fs;
use tempfile::tempdir;

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

    handle_gc_global(false, None, false, OutputFormat::Text).expect("global gc should succeed");

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

    handle_gc_global(true, None, false, OutputFormat::Text)
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

fn legacy_session_root_for(project_root: &std::path::Path) -> std::path::PathBuf {
    let normalized =
        std::fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let storage_key = normalized
        .to_string_lossy()
        .trim_start_matches('/')
        .replace('/', std::path::MAIN_SEPARATOR_STR);
    csa_config::paths::legacy_state_dir()
        .expect("legacy state dir")
        .join(storage_key)
}

fn seed_runtime_session(
    project_root: &std::path::Path,
    phase: SessionPhase,
    last_accessed: chrono::DateTime<chrono::Utc>,
    runtime_bytes: u64,
    store_in_legacy: bool,
) -> (String, std::path::PathBuf, std::path::PathBuf) {
    std::fs::create_dir_all(project_root).unwrap();

    let mut session =
        create_session(project_root, Some("gc runtime test"), None, Some("codex")).unwrap();
    session.phase = phase;
    session.last_accessed = last_accessed;
    save_session(&session).unwrap();

    let canonical_root = get_session_root(project_root).unwrap();
    let mut session_dir = canonical_root
        .join("sessions")
        .join(&session.meta_session_id);
    if store_in_legacy {
        let legacy_root = legacy_session_root_for(project_root);
        std::fs::create_dir_all(legacy_root.join("sessions")).unwrap();
        let legacy_dir = legacy_root.join("sessions").join(&session.meta_session_id);
        std::fs::rename(&session_dir, &legacy_dir).unwrap();
        session_dir = legacy_dir;
    }

    let runtime_dir = session_dir
        .join("runtime")
        .join("gemini-home")
        .join(".npm")
        .join("_cacache");
    std::fs::create_dir_all(&runtime_dir).unwrap();
    let cache_blob = runtime_dir.join("blob.bin");
    let file = std::fs::File::create(&cache_blob).unwrap();
    file.set_len(runtime_bytes).unwrap();

    let now = chrono::Utc::now();
    save_result(
        project_root,
        &session.meta_session_id,
        &SessionResult {
            status: "success".to_string(),
            exit_code: 0,
            summary: "completed".to_string(),
            tool: "codex".to_string(),
            started_at: now,
            completed_at: now,
            events_count: 0,
            artifacts: vec![SessionArtifact::new("output/summary.md")],
            peak_memory_mb: None,
            manager_fields: Default::default(),
        },
    )
    .unwrap();
    std::fs::write(session_dir.join("stderr.log"), "stderr").unwrap();
    std::fs::write(session_dir.join("output/summary.md"), "summary").unwrap();

    (
        session.meta_session_id,
        session_dir.clone(),
        session_dir.join("runtime"),
    )
}

#[test]
fn test_reap_runtime_basic_preserves_audit_files_and_session_result() {
    const TWO_MIB: u64 = 2 * 1024 * 1024;

    let tmp = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project_root = tmp.path().join("project");
    let (session_id, session_dir, runtime_dir) = seed_runtime_session(
        &project_root,
        SessionPhase::Retired,
        chrono::Utc::now() - chrono::Duration::days(40),
        TWO_MIB,
        false,
    );
    let session_root = get_session_root(&project_root).unwrap();
    let sessions = list_sessions(&project_root, None).unwrap();

    let stats = reap_runtime_payloads_in_root(&session_root, &sessions, false, 30, None).unwrap();

    assert_eq!(stats.sessions_reaped, 1);
    assert_eq!(stats.bytes_reclaimed, TWO_MIB);
    assert!(!runtime_dir.exists(), "runtime/ should be removed");
    assert!(session_dir.join("state.toml").exists());
    assert!(session_dir.join("metadata.toml").exists());
    assert!(session_dir.join("result.toml").exists());
    assert!(session_dir.join("stderr.log").exists());
    assert!(session_dir.join("output").exists());
    handle_session_result(
        session_id,
        false,
        Some(project_root.to_string_lossy().to_string()),
        StructuredOutputOpts::default(),
    )
    .expect("csa session result should still work after runtime reap");
}

#[test]
fn test_reap_runtime_skips_active_session() {
    const TWO_MIB: u64 = 2 * 1024 * 1024;

    let tmp = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project_root = tmp.path().join("project");
    let (_, _, runtime_dir) = seed_runtime_session(
        &project_root,
        SessionPhase::Active,
        chrono::Utc::now() - chrono::Duration::days(40),
        TWO_MIB,
        false,
    );
    let session_root = get_session_root(&project_root).unwrap();
    let sessions = list_sessions(&project_root, None).unwrap();

    let stats = reap_runtime_payloads_in_root(&session_root, &sessions, false, 30, None).unwrap();

    assert_eq!(stats.sessions_reaped, 0);
    assert!(
        runtime_dir.exists(),
        "active session runtime/ must be preserved"
    );
}

#[test]
fn test_reap_runtime_skips_current_session() {
    const TWO_MIB: u64 = 2 * 1024 * 1024;

    let tmp = tempdir().unwrap();
    let mut sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    sandbox.track_env("CSA_SESSION_ID");
    let project_root = tmp.path().join("project");
    let (session_id, _, runtime_dir) = seed_runtime_session(
        &project_root,
        SessionPhase::Retired,
        chrono::Utc::now() - chrono::Duration::days(40),
        TWO_MIB,
        false,
    );
    let session_root = get_session_root(&project_root).unwrap();
    let sessions = list_sessions(&project_root, None).unwrap();
    // SAFETY: test-scoped env mutation while ScopedSessionSandbox holds TEST_ENV_LOCK.
    unsafe {
        std::env::set_var("CSA_SESSION_ID", &session_id);
    }

    let stats = reap_runtime_payloads_in_root(
        &session_root,
        &sessions,
        false,
        30,
        std::env::var("CSA_SESSION_ID").ok().as_deref(),
    )
    .unwrap();

    assert_eq!(stats.sessions_reaped, 0);
    assert!(
        runtime_dir.exists(),
        "current session runtime/ must be preserved"
    );
}

#[test]
fn test_reap_runtime_respects_max_age_days() {
    const TWO_MIB: u64 = 2 * 1024 * 1024;

    let tmp = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project_root = tmp.path().join("project");
    let (_, _, runtime_dir) = seed_runtime_session(
        &project_root,
        SessionPhase::Retired,
        chrono::Utc::now() - chrono::Duration::days(5),
        TWO_MIB,
        false,
    );
    let session_root = get_session_root(&project_root).unwrap();
    let sessions = list_sessions(&project_root, None).unwrap();

    let stats = reap_runtime_payloads_in_root(&session_root, &sessions, false, 30, None).unwrap();

    assert_eq!(stats.sessions_reaped, 0);
    assert!(
        runtime_dir.exists(),
        "recent retired session should be skipped"
    );
}

#[test]
fn test_reap_runtime_dry_run_reports_bytes_without_deleting() {
    const TWO_MIB: u64 = 2 * 1024 * 1024;

    let tmp = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project_root = tmp.path().join("project");
    let (_, _, runtime_dir) = seed_runtime_session(
        &project_root,
        SessionPhase::Retired,
        chrono::Utc::now() - chrono::Duration::days(40),
        TWO_MIB,
        false,
    );
    let session_root = get_session_root(&project_root).unwrap();
    let sessions = list_sessions(&project_root, None).unwrap();

    let stats = reap_runtime_payloads_in_root(&session_root, &sessions, true, 30, None).unwrap();

    assert_eq!(stats.sessions_reaped, 1);
    assert_eq!(stats.bytes_reclaimed, TWO_MIB);
    assert!(runtime_dir.exists(), "dry-run must not delete runtime/");
}

#[test]
fn test_reap_runtime_global_covers_canonical_and_legacy_roots() {
    const TWO_MIB: u64 = 2 * 1024 * 1024;

    let tmp = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let canonical_project = tmp.path().join("canonical-project");
    let legacy_project = tmp.path().join("legacy-project");
    let (_, _, canonical_runtime) = seed_runtime_session(
        &canonical_project,
        SessionPhase::Retired,
        chrono::Utc::now() - chrono::Duration::days(40),
        TWO_MIB,
        false,
    );
    let (_, _, legacy_runtime) = seed_runtime_session(
        &legacy_project,
        SessionPhase::Retired,
        chrono::Utc::now() - chrono::Duration::days(40),
        TWO_MIB,
        true,
    );

    let stats = reap_runtime_payloads_global(false, 30).unwrap();

    assert_eq!(stats.sessions_reaped, 2);
    assert_eq!(stats.bytes_reclaimed, 2 * TWO_MIB);
    assert!(!canonical_runtime.exists());
    assert!(!legacy_runtime.exists());
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
    use csa_session::state::{PhaseEvent, SessionPhase};

    let now = chrono::Utc::now();

    // Case 1: Old Active → eligible
    let stale = now - chrono::Duration::days(10);
    let age = now.signed_duration_since(stale);
    let phase = SessionPhase::Active;
    assert!(age.num_days() > RETIRE_AFTER_DAYS && phase.transition(&PhaseEvent::Retired).is_ok());

    // Case 2: Young Active → not eligible (age guard fails)
    let recent = now - chrono::Duration::days(3);
    let age = now.signed_duration_since(recent);
    assert!(
        !(age.num_days() > RETIRE_AFTER_DAYS && phase.transition(&PhaseEvent::Retired).is_ok())
    );

    // Case 3: Old Retired → not eligible (phase guard fails)
    let stale = now - chrono::Duration::days(10);
    let age = now.signed_duration_since(stale);
    let phase = SessionPhase::Retired;
    assert!(
        !(age.num_days() > RETIRE_AFTER_DAYS && phase.transition(&PhaseEvent::Retired).is_ok())
    );
}
