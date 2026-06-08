use super::{StructuredOutputOpts, handle_session_result};
use crate::test_env_lock::TEST_ENV_LOCK;
use csa_session::{SessionResult, create_session, get_session_dir, save_result};
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

fn create_pending_tier_failover_session(
    project: &std::path::Path,
    label: &str,
) -> anyhow::Result<String> {
    let session = create_session(project, Some(label), None, Some("gemini-cli"))?;
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id)?;
    save_result(
        project,
        &session_id,
        &SessionResult {
            status: "tier_failover_superseded".to_string(),
            exit_code: 1,
            summary: "status: 400".to_string(),
            tool: "gemini-cli".to_string(),
            ..Default::default()
        },
    )?;
    std::fs::write(
        session_dir.join("stderr.log"),
        "fallback codex review is being scheduled\n",
    )?;

    let output_dir = session_dir.join("output");
    std::fs::create_dir_all(&output_dir)?;
    std::fs::write(output_dir.join("index.toml"), "not = [valid")?;
    Ok(session_id)
}

#[test]
fn structured_flags_report_pending_tier_failover_before_reading_sections() {
    let tmp = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = tmp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", tmp.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = tmp.path();

    for (label, structured) in [
        (
            "summary",
            StructuredOutputOpts {
                summary: true,
                ..Default::default()
            },
        ),
        (
            "section",
            StructuredOutputOpts {
                section: Some("details".to_string()),
                ..Default::default()
            },
        ),
        (
            "full",
            StructuredOutputOpts {
                full: true,
                ..Default::default()
            },
        ),
    ] {
        let session_id =
            create_pending_tier_failover_session(project, &format!("result-pending-{label}"))
                .unwrap();

        handle_session_result(
            session_id,
            false,
            Some(project.to_string_lossy().into_owned()),
            structured,
        )
        .unwrap_or_else(|err| panic!("{label} structured result should stay pending: {err}"));
    }
}
