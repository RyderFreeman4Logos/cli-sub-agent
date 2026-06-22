use super::{StructuredOutputOpts, handle_session_result};
use crate::test_env_lock::TEST_ENV_LOCK;
use csa_session::{create_session, get_session_dir};
use tempfile::tempdir;

struct EnvVarGuard {
    key: &'static str,
    original: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
        unsafe {
            match self.original.as_deref() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn handle_session_result_uses_metadata_only_exact_fallback_for_started_id() {
    let tmp = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = tmp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", tmp.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let caller_project = tmp.path().join("caller");
    let owner_project = tmp.path().join("owner");
    std::fs::create_dir_all(&caller_project).unwrap();
    std::fs::create_dir_all(&owner_project).unwrap();

    let session = create_session(
        &owner_project,
        Some("result-metadata-only"),
        None,
        Some("codex"),
    )
    .unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(&owner_project, &session_id).unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    std::fs::write(
        session_dir.join(csa_session::result::RESULT_FILE_NAME),
        format!(
            "status = \"success\"\nexit_code = 0\nsummary = \"metadata-only exact fallback result\"\ntool = \"codex\"\nstarted_at = \"{now}\"\ncompleted_at = \"{now}\"\n"
        ),
    )
    .unwrap();
    std::fs::remove_file(session_dir.join("state.toml")).unwrap();

    handle_session_result(
        session_id,
        false,
        Some(caller_project.to_string_lossy().into_owned()),
        StructuredOutputOpts::default(),
    )
    .expect("session result should resolve a CSA:SESSION_STARTED id via metadata fallback");

    assert!(
        session_dir
            .join(csa_session::result::RESULT_FILE_NAME)
            .is_file(),
        "result fallback must use the durable result artifact without requiring state.toml"
    );
}
