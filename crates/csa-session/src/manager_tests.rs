use super::*;
use crate::test_env::TEST_ENV_LOCK;
use tempfile::tempdir;

/// RAII guard that redirects `XDG_STATE_HOME` into a temp directory and clears
/// daemon-inherited env vars (`CSA_DAEMON_SESSION_ID`, `CSA_DAEMON_SESSION_DIR`,
/// `CSA_DAEMON_PROJECT_ROOT`) so tests don't collide with a live daemon or the
/// real state directory.  The process-wide `TEST_ENV_LOCK` serialises all
/// env-mutating tests.
struct ScopedXdgOverride {
    orig_xdg: Option<String>,
    orig_daemon_id: Option<String>,
    orig_daemon_dir: Option<String>,
    orig_daemon_root: Option<String>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl ScopedXdgOverride {
    fn new(tmp: &tempfile::TempDir) -> Self {
        let lock = TEST_ENV_LOCK.lock().expect("env lock poisoned");
        let orig_xdg = std::env::var("XDG_STATE_HOME").ok();
        let orig_daemon_id = std::env::var("CSA_DAEMON_SESSION_ID").ok();
        let orig_daemon_dir = std::env::var("CSA_DAEMON_SESSION_DIR").ok();
        let orig_daemon_root = std::env::var("CSA_DAEMON_PROJECT_ROOT").ok();
        // SAFETY: test-scoped env mutation protected by TEST_ENV_LOCK.
        unsafe {
            std::env::set_var("XDG_STATE_HOME", tmp.path().join("state").to_str().unwrap());
            std::env::remove_var("CSA_DAEMON_SESSION_ID");
            std::env::remove_var("CSA_DAEMON_SESSION_DIR");
            std::env::remove_var("CSA_DAEMON_PROJECT_ROOT");
        }
        Self {
            orig_xdg,
            orig_daemon_id,
            orig_daemon_dir,
            orig_daemon_root,
            _lock: lock,
        }
    }
}

impl Drop for ScopedXdgOverride {
    fn drop(&mut self) {
        // SAFETY: restoration of test-scoped env mutation (lock still held).
        unsafe {
            restore_env("XDG_STATE_HOME", &self.orig_xdg);
            restore_env("CSA_DAEMON_SESSION_ID", &self.orig_daemon_id);
            restore_env("CSA_DAEMON_SESSION_DIR", &self.orig_daemon_dir);
            restore_env("CSA_DAEMON_PROJECT_ROOT", &self.orig_daemon_root);
        }
    }
}

/// # Safety
/// Caller must hold `TEST_ENV_LOCK`.
unsafe fn restore_env(key: &str, original: &Option<String>) {
    // SAFETY: caller holds TEST_ENV_LOCK, ensuring serial access.
    unsafe {
        match original {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
}

struct EnvVarGuard {
    key: &'static str,
    original: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation is guarded by TEST_ENV_LOCK.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }

    fn unset(key: &'static str) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation is guarded by TEST_ENV_LOCK.
        unsafe { std::env::remove_var(key) };
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: test-scoped env restoration is guarded by TEST_ENV_LOCK.
        unsafe {
            match self.original.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

#[test]
fn test_create_session() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), Some("Test session"), None, None).unwrap();
    assert_eq!(state.description, Some("Test session".to_string()));
    assert_eq!(state.genealogy.depth, 0);
    assert!(state.genealogy.parent_session_id.is_none());
    let dir = get_session_dir_in(td.path(), &state.meta_session_id);
    assert!(dir.exists());
    assert!(dir.join(STATE_FILE_NAME).exists());
    assert!(dir.join("input").is_dir());
    assert!(dir.join("output").is_dir());
}

#[test]
fn test_load_session() {
    let td = tempdir().unwrap();
    let created = create_session_in(td.path(), td.path(), Some("Test"), None, None).unwrap();
    let loaded = load_session_in(td.path(), &created.meta_session_id).unwrap();
    assert_eq!(loaded.meta_session_id, created.meta_session_id);
    assert_eq!(loaded.description, created.description);
}

#[test]
fn test_delete_session() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), None, None, None).unwrap();
    let dir = get_session_dir_in(td.path(), &state.meta_session_id);
    assert!(dir.exists());
    delete_session_in(td.path(), &state.meta_session_id).unwrap();
    assert!(!dir.exists());
}

#[test]
fn test_list_all_sessions() {
    let td = tempdir().unwrap();
    let _xdg = ScopedXdgOverride::new(&td);
    create_session_in(td.path(), td.path(), Some("S1"), None, None).unwrap();
    create_session_in(td.path(), td.path(), Some("S2"), None, None).unwrap();
    assert_eq!(list_all_sessions_in(td.path()).unwrap().len(), 2);
}

#[test]
fn test_create_session_ignores_bare_inherited_daemon_session_id() {
    let _env_lock = TEST_ENV_LOCK.lock().unwrap();
    let _daemon_session_id =
        EnvVarGuard::set("CSA_DAEMON_SESSION_ID", "01K00000000000000000000000");
    let _daemon_session_dir = EnvVarGuard::unset("CSA_DAEMON_SESSION_DIR");
    let _daemon_project_root = EnvVarGuard::unset("CSA_DAEMON_PROJECT_ROOT");

    let td = tempdir().unwrap();
    let first = create_session_in(td.path(), td.path(), Some("S1"), None, None).unwrap();
    let second = create_session_in(td.path(), td.path(), Some("S2"), None, None).unwrap();

    assert_ne!(first.meta_session_id, second.meta_session_id);
    assert_eq!(list_all_sessions_in(td.path()).unwrap().len(), 2);
}

#[test]
fn test_list_sessions_with_tool_filter() {
    let td = tempdir().unwrap();
    let _xdg = ScopedXdgOverride::new(&td);
    let mut s1 = create_session_in(td.path(), td.path(), Some("S1"), None, None).unwrap();
    s1.tools.insert(
        "codex".to_string(),
        crate::state::ToolState {
            provider_session_id: Some("thread_123".to_string()),
            last_action_summary: "Test".to_string(),
            last_exit_code: 0,
            updated_at: Utc::now(),
            token_usage: None,
        },
    );
    save_session_in(td.path(), &s1).unwrap();
    create_session_in(td.path(), td.path(), Some("S2"), None, None).unwrap();
    let filtered = list_sessions_in(td.path(), Some(&["codex"])).unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].meta_session_id, s1.meta_session_id);
}

#[test]
fn test_resolve_resume_session_with_provider_id() {
    let td = tempdir().unwrap();
    let mut state = create_session_in(td.path(), td.path(), Some("Resume"), None, None).unwrap();
    state.tools.insert(
        "codex".to_string(),
        crate::state::ToolState {
            provider_session_id: Some("provider_session_123".to_string()),
            last_action_summary: "resume".to_string(),
            last_exit_code: 0,
            updated_at: Utc::now(),
            token_usage: None,
        },
    );
    save_session_in(td.path(), &state).unwrap();

    let prefix = &state.meta_session_id[..10];
    let resolved = resolve_resume_session_in(td.path(), prefix, "codex").unwrap();

    assert_eq!(resolved.meta_session_id, state.meta_session_id);
    assert_eq!(
        resolved.provider_session_id,
        Some("provider_session_123".to_string())
    );
}

#[test]
fn test_resolve_resume_session_without_provider_id() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), Some("Resume"), None, None).unwrap();

    let resolved = resolve_resume_session_in(td.path(), &state.meta_session_id, "codex").unwrap();
    assert_eq!(resolved.meta_session_id, state.meta_session_id);
    assert!(resolved.provider_session_id.is_none());
}

#[test]
fn test_resolve_resume_session_respects_tool_lock() {
    let td = tempdir().unwrap();
    let state =
        create_session_in(td.path(), td.path(), Some("Locked"), None, Some("codex")).unwrap();

    let err =
        resolve_resume_session_in(td.path(), &state.meta_session_id, "gemini-cli").unwrap_err();
    assert!(err.to_string().contains("locked to tool"));
}

#[test]
fn test_create_child_session() {
    let td = tempdir().unwrap();
    let parent = create_session_in(td.path(), td.path(), Some("Parent"), None, None).unwrap();
    let child = create_session_in(
        td.path(),
        td.path(),
        Some("Child"),
        Some(&parent.meta_session_id),
        None,
    )
    .unwrap();
    assert_eq!(
        child.genealogy.parent_session_id,
        Some(parent.meta_session_id.clone())
    );
    assert_eq!(child.genealogy.depth, 1);
}

#[test]
fn test_round_trip() {
    let td = tempdir().unwrap();
    let created = create_session_in(td.path(), td.path(), Some("Round trip"), None, None).unwrap();
    let loaded = load_session_in(td.path(), &created.meta_session_id).unwrap();
    assert_eq!(loaded.meta_session_id, created.meta_session_id);
    assert_eq!(loaded.description, created.description);
    assert_eq!(loaded.project_path, created.project_path);
    assert_eq!(loaded.genealogy.depth, created.genealogy.depth);
}

#[test]
fn test_create_session_with_tool() {
    let td = tempdir().unwrap();
    let state = create_session_in(td.path(), td.path(), Some("Test"), None, Some("codex")).unwrap();
    let dir = get_session_dir_in(td.path(), &state.meta_session_id);
    assert!(dir.join("metadata.toml").exists());
    let meta = load_metadata_in(td.path(), &state.meta_session_id)
        .unwrap()
        .unwrap();
    assert_eq!(meta.tool, "codex");
    assert!(meta.tool_locked);
}

include!("manager_tests_tail.rs");
include!("manager_tests_audit.rs");
include!("manager_tests_result_view.rs");
