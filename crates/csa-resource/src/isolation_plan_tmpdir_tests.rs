use super::*;

use std::ffi::OsString;

struct ScopedEnvVar {
    key: &'static str,
    previous: Option<OsString>,
}

impl ScopedEnvVar {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: tests that mutate process environment hold ENV_LOCK, so no
        // other test in this module observes a concurrent environment change.
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        // SAFETY: the guard is only used while ENV_LOCK is held, preserving
        // exclusive access to process environment mutations for these tests.
        unsafe {
            if let Some(value) = &self.previous {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}

#[test]
fn non_bwrap_tool_defaults_create_writable_session_tmpdir() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    let project = temp.path().join("project");
    std::fs::create_dir_all(&home).expect("create home");
    std::fs::create_dir_all(&project).expect("create project");
    let _home = ScopedEnvVar::set("HOME", &home);

    for (filesystem, label) in [
        (FilesystemCapability::Landlock, "landlock"),
        (FilesystemCapability::None, "none"),
    ] {
        let session = temp.path().join(format!("session-{label}"));
        std::fs::create_dir_all(&session).expect("create session dir");
        let session_tmp = session.join("tmp");

        let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
            .with_filesystem_capability(filesystem)
            .with_tool_defaults("codex", &project, &session)
            .build()
            .expect("should build isolation plan");

        assert_eq!(
            plan.env_overrides.get("TMPDIR"),
            Some(&session_tmp.to_string_lossy().into_owned()),
            "{label} sessions should pin TMPDIR to a session-owned dir"
        );
        assert!(
            plan.writable_paths.contains(&session_tmp),
            "{label} session TMPDIR must be included in writable paths"
        );
        assert!(
            session_tmp.is_dir(),
            "{label} session TMPDIR must exist before child hooks/tools run"
        );
        std::fs::write(session_tmp.join("probe"), "ok").expect("TMPDIR should be writable");
    }
}
