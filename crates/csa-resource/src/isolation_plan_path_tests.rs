use super::*;
use std::ffi::OsString;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct ScopedEnvVar {
    key: &'static str,
    previous: Option<OsString>,
}

impl ScopedEnvVar {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: tests that mutate process environment hold ENV_LOCK.
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        // SAFETY: tests that mutate process environment hold ENV_LOCK.
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
fn test_resolve_writable_relative_path_against_project_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let drafts = project.join("drafts");
    std::fs::create_dir_all(&drafts).expect("create drafts dir");

    let resolved =
        resolve_writable_paths(&[PathBuf::from("./drafts")], &project).expect("valid path");

    assert_eq!(resolved, vec![drafts.canonicalize().unwrap()]);
}

#[cfg(unix)]
#[test]
fn test_resolve_writable_symlink_inside_project_to_external_target() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let external = tmp.path().join("external-drafts");
    std::fs::create_dir_all(&project).expect("create project dir");
    std::fs::create_dir_all(&external).expect("create external dir");
    symlink(&external, project.join("drafts")).expect("create symlink");

    let resolved = resolve_writable_paths(&[PathBuf::from("drafts")], &project)
        .expect("project-local symlink should be accepted");

    let canonical_project = project.canonicalize().unwrap();
    let canonical_external = external.canonicalize().unwrap();
    assert_eq!(resolved, vec![canonical_external.clone()]);
    assert!(!canonical_external.starts_with(canonical_project));
}

#[test]
fn test_resolve_writable_allows_nonexistent_path_with_existing_parent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).expect("create project dir");

    let resolved = resolve_writable_paths(&[PathBuf::from("drafts/new")], &project)
        .expect("generic writable_paths may target a creatable child path");

    assert_eq!(resolved, vec![project.join("drafts/new")]);
}

#[test]
fn test_resolve_writable_accepts_config_path_outside_default_roots() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let external = tmp.path().join("external-data");
    std::fs::create_dir_all(&project).expect("create project dir");
    std::fs::create_dir_all(&external).expect("create external dir");

    let resolved = resolve_writable_paths(std::slice::from_ref(&external), &project)
        .expect("config extra_writable outside default roots should be accepted");

    assert_eq!(resolved, vec![external.canonicalize().unwrap()]);
}

#[cfg(unix)]
#[test]
fn test_writable_validation_error_includes_original_and_resolved_path() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).expect("create project dir");
    symlink("/etc", project.join("etc-link")).expect("create symlink");

    let err = resolve_writable_paths(&[PathBuf::from("etc-link")], &project)
        .expect_err("sensitive symlink target should be rejected")
        .to_string();

    assert!(err.contains("etc-link"), "missing original path: {err}");
    assert!(
        err.contains("resolved path /etc is forbidden"),
        "missing resolved path: {err}"
    );
}

#[test]
fn test_claude_tool_defaults_precreate_claude_dir_for_session_env() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).expect("create home");
    let _home_env = ScopedEnvVar::set("HOME", &home);

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults(
            "claude-code",
            std::path::Path::new("/tmp/project"),
            std::path::Path::new("/tmp/session"),
        )
        .build()
        .expect("should succeed");

    let claude_dir = home.join(".claude");
    assert!(claude_dir.is_dir());
    assert!(plan.writable_paths.contains(&claude_dir));
}
