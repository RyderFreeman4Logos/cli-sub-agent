//! Plan-specific bind-FD capability regressions (#3148).

use super::*;
use std::path::Path;

#[cfg(unix)]
struct PathGuard(Option<std::ffi::OsString>);

#[cfg(unix)]
impl Drop for PathGuard {
    fn drop(&mut self) {
        // SAFETY: crate ENV_LOCK serializes process-environment mutation.
        unsafe {
            match &self.0 {
                Some(path) => std::env::set_var("PATH", path),
                None => std::env::remove_var("PATH"),
            }
        }
    }
}

#[cfg(unix)]
fn install_legacy_bwrap(temp: &tempfile::TempDir) -> PathGuard {
    use std::os::unix::fs::PermissionsExt;

    let bwrap = temp.path().join("bwrap");
    std::fs::write(
        &bwrap,
        "#!/bin/sh\n[ \"$1\" = --help ] && { echo 'usage: bwrap --ro-bind SRC DEST --bind SRC DEST'; exit 0; }\nexit 64\n",
    )
    .unwrap();
    std::fs::set_permissions(&bwrap, std::fs::Permissions::from_mode(0o755)).unwrap();
    let unshare = temp.path().join("unshare");
    std::fs::write(&unshare, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&unshare, std::fs::Permissions::from_mode(0o755)).unwrap();
    let old_path = std::env::var_os("PATH");
    let path = format!(
        "{}:{}",
        temp.path().display(),
        old_path.as_deref().unwrap_or_default().to_string_lossy()
    );
    // SAFETY: crate ENV_LOCK serializes process-environment mutation.
    unsafe { std::env::set_var("PATH", &path) };
    PathGuard(old_path)
}

#[cfg(unix)]
#[test]
fn legacy_bwrap_keeps_ordinary_plans_and_fails_closed_for_hermes_bind_fd() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("legacy-bwrap-plan-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let (_home, _env) = isolated_home(&temp);
    let _path = install_legacy_bwrap(&temp);
    assert!(
        !crate::filesystem_sandbox::has_bwrap_bind_fd_options(),
        "legacy bwrap must not report descriptor-bind support"
    );
    let capability = FilesystemCapability::Bwrap;

    let project = temp.path().join("project");
    let session = temp.path().join("session");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&session).unwrap();

    for tool in ["codex", "claude-code", "gemini-cli", "opencode"] {
        let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
            .with_filesystem_capability(capability)
            .with_tool_defaults(tool, &project, &session)
            .build()
            .unwrap_or_else(|error| panic!("{tool} ordinary plan must build: {error:#}"));
        assert_eq!(
            plan.filesystem,
            FilesystemCapability::Bwrap,
            "{tool} must keep baseline bwrap without descriptor binds"
        );
        assert_eq!(
            crate::bwrap::sandbox_bind_fd_count(&plan),
            0,
            "{tool} must not require bind-FD support"
        );
    }

    let hermes_home = temp.path().join("hermes-home");
    std::fs::create_dir_all(hermes_home.join("logs")).unwrap();
    std::fs::write(hermes_home.join("config.yaml"), "model: test\n").unwrap();
    let execution_env = std::collections::HashMap::from([(
        "HERMES_HOME".to_string(),
        hermes_home.to_string_lossy().into_owned(),
    )]);
    let error = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(capability)
        .with_execution_env(Some(&execution_env))
        .with_tool_defaults("hermes", &project, &session)
        .build()
        .expect_err("Hermes descriptor-bind plan must fail closed without bind-FD support");
    assert!(
        error.to_string().contains("bind-fd"),
        "Hermes bind-FD failure must identify missing descriptor binds: {error:#}"
    );
    assert!(
        Path::new(&hermes_home)
            .join(".csa-runtime/.csa-runtime-ready")
            .symlink_metadata()
            .is_err(),
        "failed Hermes bind-FD plan must not activate runtime"
    );
}
