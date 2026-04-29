use super::*;
use crate::session_cmds_result::{StructuredOutputOpts, handle_session_result};
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_core::types::OutputFormat;
use csa_session::{
    SessionArtifact, SessionPhase, SessionResult, ToolState, create_session, get_session_root,
    list_sessions, save_result, save_session,
};
use std::io;
use std::sync::{Arc, LazyLock, Mutex};
use tempfile::tempdir;
use tracing_subscriber::fmt::MakeWriter;

static CURRENT_DIR_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(Clone, Default)]
struct SharedLogBuffer {
    inner: Arc<Mutex<Vec<u8>>>,
}

impl SharedLogBuffer {
    fn contents(&self) -> String {
        String::from_utf8(self.inner.lock().expect("log buffer poisoned").clone())
            .expect("log buffer should be valid UTF-8")
    }
}

impl<'a> MakeWriter<'a> for SharedLogBuffer {
    type Writer = SharedLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        SharedLogWriter {
            inner: Arc::clone(&self.inner),
        }
    }
}

struct SharedLogWriter {
    inner: Arc<Mutex<Vec<u8>>>,
}

impl io::Write for SharedLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner
            .lock()
            .expect("log buffer poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct CurrentDirGuard {
    original: std::path::PathBuf,
}

impl CurrentDirGuard {
    fn enter(path: &std::path::Path) -> Self {
        let original = std::env::current_dir().expect("read current dir");
        std::env::set_current_dir(path).expect("set current dir");
        Self { original }
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.original).expect("restore current dir");
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
    session.tools.insert(
        "codex".to_string(),
        ToolState {
            provider_session_id: Some("provider-session".to_string()),
            last_action_summary: "completed".to_string(),
            last_exit_code: 0,
            updated_at: last_accessed,
            tool_version: None,
            token_usage: None,
        },
    );
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
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
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
fn test_reap_runtime_dry_run_reports_runtime_for_session_retired_in_same_run() {
    const TWO_MIB: u64 = 2 * 1024 * 1024;

    let tmp = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let project_root = tmp.path().join("project");
    let (session_id, _, runtime_dir) = seed_runtime_session(
        &project_root,
        SessionPhase::Active,
        chrono::Utc::now() - chrono::Duration::days(RETIRE_AFTER_DAYS + 1),
        TWO_MIB,
        false,
    );
    let session_root = get_session_root(&project_root).unwrap();
    let sessions = list_sessions(&project_root, None).unwrap();
    assert_eq!(sessions[0].phase, SessionPhase::Active);
    let dry_run_sessions = super::reaper::sessions_with_dry_run_retirements(
        &sessions,
        chrono::Utc::now(),
        RETIRE_AFTER_DAYS,
    );

    let stats = reap_runtime_payloads_in_root(
        &session_root,
        &dry_run_sessions,
        true,
        RETIRE_AFTER_DAYS as u64,
        None,
    )
    .unwrap();

    assert_eq!(stats.sessions_reaped, 1);
    assert_eq!(stats.bytes_reclaimed, TWO_MIB);
    assert!(
        stats.entries.iter().any(|entry| {
            entry.session_id == session_id
                && entry.runtime_path == runtime_dir.display().to_string()
        }),
        "dry-run runtime reap output must include sessions that will be retired in the same run"
    );
    assert!(runtime_dir.exists(), "dry-run must not delete runtime/");
    let sessions_after = list_sessions(&project_root, None).unwrap();
    assert_eq!(sessions_after[0].phase, SessionPhase::Active);
}

#[test]
fn test_reap_runtime_skips_active_flock_and_warns() {
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
    let _lock = csa_lock::acquire_lock(&session_dir, "gemini-cli", "test active lock").unwrap();
    let session_root = get_session_root(&project_root).unwrap();
    let sessions = list_sessions(&project_root, None).unwrap();

    let buffer = SharedLogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(buffer.clone())
        .without_time()
        .finish();
    let stats = tracing::subscriber::with_default(subscriber, || {
        reap_runtime_payloads_in_root(&session_root, &sessions, false, 30, None).unwrap()
    });

    assert_eq!(stats.sessions_reaped, 0);
    assert!(runtime_dir.exists(), "locked session runtime/ must remain");
    let logs = buffer.contents();
    assert!(logs.contains("Skipping runtime reap for locked session"));
    assert!(logs.contains(&session_id));
}

#[test]
fn test_reap_runtime_dirs_config_false_skips_cleanup() {
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
    let gc_config = csa_config::GcConfig {
        reap_runtime_dirs: false,
        ..Default::default()
    };
    let session_root = get_session_root(&project_root).unwrap();
    let sessions = list_sessions(&project_root, None).unwrap();

    let max_age_days = runtime_reap_max_age_days(false, None, gc_config).unwrap();
    let stats = max_age_days
        .map(|days| reap_runtime_payloads_in_root(&session_root, &sessions, false, days, None))
        .transpose()
        .unwrap();

    assert_eq!(stats, None);
    assert!(
        runtime_dir.exists(),
        "reap_runtime_dirs=false must preserve runtime/"
    );
}

#[test]
fn test_handle_gc_reaps_runtime_after_retiring_stale_session() {
    const TWO_MIB: u64 = 2 * 1024 * 1024;

    let tmp = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let _cwd_lock = CURRENT_DIR_LOCK.lock().expect("current dir lock");
    let project_root = tmp.path().join("project");
    let (session_id, _, runtime_dir) = seed_runtime_session(
        &project_root,
        SessionPhase::Active,
        chrono::Utc::now() - chrono::Duration::days(RETIRE_AFTER_DAYS + 1),
        TWO_MIB,
        false,
    );
    let _cwd = CurrentDirGuard::enter(&project_root);

    handle_gc(false, None, false, OutputFormat::Text).unwrap();

    let sessions = list_sessions(&project_root, None).unwrap();
    let session = sessions
        .iter()
        .find(|session| session.meta_session_id == session_id)
        .expect("session should remain for audit");
    assert_eq!(session.phase, SessionPhase::Retired);
    assert!(
        !runtime_dir.exists(),
        "default csa gc must reap runtime/ once a stale session is retired"
    );
}

#[test]
fn test_handle_gc_respects_reap_runtime_dirs_false() {
    const TWO_MIB: u64 = 2 * 1024 * 1024;

    let tmp = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let _cwd_lock = CURRENT_DIR_LOCK.lock().expect("current dir lock");
    let project_root = tmp.path().join("project");
    let csa_config_dir = project_root.join(".csa");
    std::fs::create_dir_all(&csa_config_dir).unwrap();
    std::fs::write(
        csa_config_dir.join("config.toml"),
        "[gc]\nreap_runtime_dirs = false\n",
    )
    .unwrap();
    let (_, _, runtime_dir) = seed_runtime_session(
        &project_root,
        SessionPhase::Retired,
        chrono::Utc::now() - chrono::Duration::days(40),
        TWO_MIB,
        false,
    );
    let _cwd = CurrentDirGuard::enter(&project_root);

    handle_gc(false, None, false, OutputFormat::Text).unwrap();

    assert!(
        runtime_dir.exists(),
        "reap_runtime_dirs=false must preserve runtime/ during csa gc"
    );
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
