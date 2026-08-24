use super::*;
use std::ffi::OsString;
use std::path::Path;

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

    assert_eq!(
        resolved,
        vec![project.canonicalize().unwrap().join("drafts/new")]
    );
}

#[test]
fn test_validate_readable_paths_accepts_project_local_relative_path() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let context_file = project.join(".csa").join("review-context.md");
    std::fs::create_dir_all(context_file.parent().expect("context parent dir"))
        .expect("create .csa dir");
    std::fs::write(&context_file, "context").expect("write context file");

    validate_readable_paths(&[PathBuf::from(".csa/review-context.md")], &project).expect(
        "project-local relative readable path should resolve against project root and be accepted",
    );
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
fn test_validate_readable_paths_allows_ssd_mirror_of_home_and_project() {
    use std::os::unix::fs::symlink;

    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    let project = temp.path().join("project");
    let mirror_root = temp.path().join("mirror-rootfs");
    let mirror_home = mirror_root.join(home.strip_prefix("/").expect("absolute home"));
    let mirror_project = mirror_root.join(project.strip_prefix("/").expect("absolute project"));
    let lexical_tmp = home.join("tmp");
    let mirror_tmp = mirror_home.join("tmp");
    let lexical_project_data = project.join("data");
    let mirror_project_data = mirror_project.join("data");
    std::fs::create_dir_all(&home).expect("create home");
    std::fs::create_dir_all(&project).expect("create project");
    std::fs::create_dir_all(&mirror_tmp).expect("create mirror tmp");
    std::fs::create_dir_all(&mirror_project_data).expect("create mirror project data");
    symlink(&mirror_tmp, &lexical_tmp).expect("link home tmp to mirror");
    symlink(&mirror_project_data, &lexical_project_data).expect("link project data to mirror");
    let _home_env = ScopedEnvVar::set("HOME", &home);

    super::validation::validate_readable_paths_with_mirror_roots(
        &[lexical_tmp, mirror_tmp, lexical_project_data],
        &project,
        std::slice::from_ref(&mirror_root),
    )
    .expect("home paths resolved through the SSD mirror should be readable");
}

#[cfg(unix)]
#[test]
fn test_validate_readable_paths_rejects_unrelated_and_sensitive_paths_with_ssd_mirror() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    let project = home.join("project");
    let mirror_root = temp.path().join("mirror-rootfs");
    let unrelated = PathBuf::from("/var");
    std::fs::create_dir_all(&project).expect("create project");
    let _home_env = ScopedEnvVar::set("HOME", &home);

    for path in [&unrelated, Path::new("/etc")] {
        let paths = [path.to_path_buf()];
        assert!(
            super::validation::validate_readable_paths_with_mirror_roots(
                &paths,
                &project,
                std::slice::from_ref(&mirror_root),
            )
            .is_err(),
            "{path:?} must not be admitted by the SSD mirror exception"
        );
    }
}

#[cfg(unix)]
#[test]
fn test_validate_readable_paths_rejects_ssd_mirror_symlink_escapes() {
    use std::os::unix::fs::symlink;

    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home/current");
    let project = home.join("project");
    let mirror_root = temp.path().join("mirror-rootfs");
    let mirror_home = mirror_root.join(home.strip_prefix("/").expect("absolute home"));
    let mirror_etc = mirror_root.join("etc");
    let direct_escape = mirror_home.join("direct-escape");
    let mirror_lexical_escape = mirror_home.join("lexical-escape");
    let lexical_escape = home.join("lexical-escape");
    std::fs::create_dir_all(&project).expect("create project");
    std::fs::create_dir_all(&mirror_home).expect("create mirror home");
    std::fs::create_dir_all(&mirror_etc).expect("create mirror etc");
    symlink(&mirror_etc, &direct_escape).expect("create direct mirror escape");
    symlink(&mirror_etc, &mirror_lexical_escape).expect("create mirrored lexical escape");
    symlink(&mirror_lexical_escape, &lexical_escape).expect("create lexical mirror escape");
    let _home_env = ScopedEnvVar::set("HOME", &home);

    for path in [direct_escape, lexical_escape] {
        super::validation::validate_readable_paths_with_mirror_roots(
            std::slice::from_ref(&path),
            &project,
            std::slice::from_ref(&mirror_root),
        )
        .expect_err("mirror symlinks must stay in the authorized logical subtree");
    }
}

#[test]
fn test_validate_readable_paths_rejects_mnt_ssd_mirror_alias() {
    validate_readable_paths(
        &[PathBuf::from("/mnt/ssd/mirror-rootfs/home/obj")],
        Path::new("/home/obj/project"),
    )
    .expect_err("only the lexical /ssd/mirror-rootfs request root is allowed");
}

#[test]
fn test_validate_writable_paths_rejects_ssd_mirror_exception() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    let project = home.join("project");
    let mirror_home =
        Path::new("/ssd/mirror-rootfs").join(home.strip_prefix("/").expect("absolute home"));
    std::fs::create_dir_all(&project).expect("create project");
    let _home_env = ScopedEnvVar::set("HOME", &home);

    validate_writable_paths(&[mirror_home], &project)
        .expect_err("the SSD mirror exception is readable-only");
}

#[test]
fn test_validate_writable_allows_xdg_runtime_child_but_rejects_root() {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let runtime_root = tmp.path().join("run/user/1001");
    let runtime_child = runtime_root.join("just");
    std::fs::create_dir_all(&project).expect("create project dir");
    std::fs::create_dir_all(&runtime_child).expect("create runtime child dir");
    let _runtime_env = ScopedEnvVar::set("XDG_RUNTIME_DIR", &runtime_root);

    validate_writable_paths(std::slice::from_ref(&runtime_child), &project)
        .expect("scoped XDG runtime child should be valid");

    let err = validate_writable_paths(std::slice::from_ref(&runtime_root), &project)
        .expect_err("XDG runtime root is too broad");
    assert!(
        err.to_string().contains("specific child directory"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_xdg_runtime_child_helper_keeps_run_user_scope_narrow() {
    let _guard = ENV_LOCK.lock().unwrap();
    let _runtime_env = ScopedEnvVar::set("XDG_RUNTIME_DIR", "/run/user/1001");

    assert!(is_xdg_runtime_child_path(Path::new("/run/user/1001/just")));
    assert!(!is_xdg_runtime_child_path(Path::new("/run/user/1001")));
    assert!(!is_xdg_runtime_child_path(Path::new("/run/user/1002/just")));
}

#[cfg(target_os = "linux")]
#[test]
fn test_user_daemon_ipc_exposes_daemon_sockets_readonly() {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().expect("tempdir");
    let runtime_root = tmp.path().join("run/user/1001");
    let runtime_child = runtime_root.join("cli-sub-agent");
    let bus_socket = runtime_root.join("bus");
    let systemd_socket = runtime_root.join("systemd/private");
    std::fs::create_dir_all(&runtime_child).expect("create runtime child");
    std::fs::create_dir_all(systemd_socket.parent().unwrap()).expect("create systemd dir");
    std::fs::write(&bus_socket, "").expect("create bus socket placeholder");
    std::fs::write(&systemd_socket, "").expect("create systemd socket placeholder");
    let _runtime_env = ScopedEnvVar::set("XDG_RUNTIME_DIR", &runtime_root);

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_writable_path(runtime_child)
        .with_user_daemon_ipc()
        .build()
        .expect("runtime child scope with user_daemon_ipc should build");

    assert!(plan.readable_paths.contains(&bus_socket));
    assert!(plan.readable_paths.contains(&systemd_socket));
}

#[test]
fn test_daemon_sockets_not_exposed_without_user_daemon_ipc() {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().expect("tempdir");
    let runtime_root = tmp.path().join("run/user/1001");
    let bus_socket = runtime_root.join("bus");
    std::fs::create_dir_all(&runtime_root).expect("create runtime root");
    std::fs::write(&bus_socket, "").expect("create bus socket placeholder");
    let _runtime_env = ScopedEnvVar::set("XDG_RUNTIME_DIR", &runtime_root);

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .build()
        .expect("plan should build without runtime scope");

    assert!(!plan.readable_paths.contains(&bus_socket));
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
        err.contains("resolved path ") && err.contains(" is forbidden"),
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
    let claude_state_file = home.join(".claude.json");
    assert!(claude_dir.is_dir());
    assert!(
        !claude_state_file.is_dir(),
        ".claude.json must remain a file path, not a pre-created directory"
    );
    assert!(plan.writable_paths.contains(&claude_dir));
    assert!(plan.writable_paths.contains(&claude_state_file));
}
