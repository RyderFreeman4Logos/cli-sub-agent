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

#[test]
fn extra_only_binds_require_modern_bwrap_before_activation() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let home = root.join("home");
    std::fs::create_dir(&home).unwrap();
    let _home = ScopedEnvVar::set("HOME", &home);
    let _path = install_legacy_bwrap(&temp);
    let project = root.join("project");
    std::fs::create_dir(&project).unwrap();
    let ordinary = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_writable_path(project.clone())
        .build()
        .expect("ordinary pathname mounts work with standard bwrap");
    assert_eq!(crate::bwrap::sandbox_bind_fd_count(&ordinary), 0);
    for implicit in [false, true] {
        if implicit {
            std::fs::create_dir_all(home.join(".config/gh-aider")).unwrap();
        }
        let result = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
            .with_filesystem_capability(FilesystemCapability::Bwrap)
            .with_writable_path(project.clone())
            .with_project_root(&project)
            .with_readonly_project_root(!implicit)
            .build();
        let error = result.expect_err("extra-only binds require descriptor support");
        assert!(error.to_string().contains("bind-fd"), "{error:#}");
    }
}

#[test]
fn extra_only_binds_survive_command_reconstruction_and_reject_systemd() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let home = root.join("home");
    std::fs::create_dir(&home).unwrap();
    let _home = ScopedEnvVar::set("HOME", &home);
    let project = root.join("project");
    std::fs::create_dir(&project).unwrap();
    for implicit in [false, true] {
        let source = if implicit {
            home.join(".config/gh-aider")
        } else {
            project.clone()
        };
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("payload"), "accepted").unwrap();
        let mut plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
            .with_filesystem_capability(FilesystemCapability::Bwrap)
            .with_resource_capability(ResourceCapability::CgroupV2)
            .with_writable_path(project.clone())
            .with_project_root(&project)
            .with_readonly_project_root(!implicit)
            .build()
            .unwrap();
        if implicit {
            plan.env_overrides
                .insert("HOME".into(), "/sandbox-home".into());
        }
        let built = crate::from_isolation_plan(&plan, "/bin/true", &[])
            .unwrap()
            .unwrap();
        if implicit {
            let args: Vec<_> = built.get_args().collect();
            assert!(args.windows(3).any(
                |args| args[0] == "--ro-bind-fd" && args[2] == "/sandbox-home/.config/gh-aider"
            ));
        }
        // Execute the reconstructed argv with a bounded stand-in for bwrap;
        // the original Command and its pre_exec owners are gone before spawn.
        let mut probe = std::process::Command::new("/bin/sh");
        probe.args(["-c", "while [ $# -gt 0 ]; do if [ \"$1\" = --ro-bind-fd ]; then exec /bin/cat /proc/self/fd/\"$2\"/payload; fi; shift; done; exit 64", "probe"]);
        probe.args(built.get_args());
        let mut systemd = std::process::Command::new("systemd-run");
        systemd.args(built.get_args());
        drop(built);
        std::fs::rename(&source, source.with_extension("old")).unwrap();
        crate::bwrap::inherit_sandbox_bind_fds(&mut probe, &plan);
        let output = crate::bounded_command::output_with_timeout(
            probe,
            std::time::Duration::from_secs(5),
            crate::bounded_command::MAX_OUTPUT_BYTES,
        )
        .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"accepted");
        assert_eq!(plan.resource, ResourceCapability::Setrlimit);
        assert!(crate::bwrap::sandbox_bind_fd_count(&plan) > 0);
        assert!(crate::bwrap::try_inherit_sandbox_bind_fds(&mut systemd, &plan).is_err());
        std::fs::rename(source.with_extension("old"), &source).unwrap();
    }
}

#[test]
fn extra_bind_preflight_preserves_3148_activation_and_overlay_fds() {
    let _lock = ENV_LOCK.lock().unwrap();
    let layouts = [
        (None, "state.db"),
        (Some("flat"), "state.flat.db"),
        (Some("direct"), "direct/state.db"),
        (Some("nested"), "profiles/nested/state.db"),
    ];
    for (profile, relative) in layouts {
        for reject in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let home = root.join("home");
            std::fs::create_dir(&home).unwrap();
            let _home = ScopedEnvVar::set("HOME", &home);
            let hermes = root.join("hermes");
            std::fs::create_dir_all(hermes.join("logs")).unwrap();
            std::fs::write(hermes.join("config.yaml"), "model: test\n").unwrap();
            let legacy = hermes.join(relative);
            std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
            if matches!(profile, Some("direct" | "nested")) {
                std::fs::write(
                    legacy.parent().unwrap().join("config.yaml"),
                    "model: test\n",
                )
                .unwrap();
            }
            let database = rusqlite::Connection::open(&legacy).unwrap();
            database
                .execute_batch(
                    "CREATE TABLE probe(value TEXT); INSERT INTO probe VALUES ('accepted');",
                )
                .unwrap();
            let project = root.join("project");
            let session = root.join("session");
            std::fs::create_dir(&project).unwrap();
            std::fs::create_dir(&session).unwrap();
            let execution_env = std::collections::HashMap::from([(
                "HERMES_HOME".into(),
                hermes.to_string_lossy().into_owned(),
            )]);
            let builder = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
                .with_filesystem_capability(FilesystemCapability::Bwrap)
                .with_execution_env(Some(&execution_env))
                .with_tool_defaults("hermes", &project, &session)
                .with_readonly_project_root(true);
            if reject {
                // The project disappears after runtime preparation. Extra bind
                // pinning must fail before publishing the SQLite generation.
                std::fs::remove_dir(&project).unwrap();
                assert!(builder.build().is_err());
                assert_eq!(resolve_hermes_state_db(&hermes, profile), legacy);
                assert!(!hermes.join(".csa-runtime/.csa-runtime-ready").exists());
            } else {
                let plan = builder.build().unwrap();
                let resolved = resolve_hermes_state_db(&hermes, profile);
                assert_ne!(resolved, legacy);
                let published = rusqlite::Connection::open(&resolved).unwrap();
                let value: String = published
                    .query_row("SELECT value FROM probe", [], |row| row.get(0))
                    .unwrap();
                assert_eq!(value, "accepted");
                let built = crate::from_isolation_plan(&plan, "/bin/true", &[])
                    .unwrap()
                    .unwrap();
                let args: Vec<_> = built
                    .get_args()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect();
                let config = hermes.join("config.yaml");
                let fd = args
                    .windows(3)
                    .find(|args| args[0] == "--ro-bind-fd" && args[2] == config.to_string_lossy())
                    .unwrap()[1]
                    .clone();
                assert!(
                    args.windows(3)
                        .any(|args| args[0] == "--bind-fd" && args[2] == hermes.to_string_lossy())
                );
                drop(built);
                let mut probe = std::process::Command::new("/bin/cat");
                probe.arg(format!("/proc/self/fd/{fd}"));
                crate::bwrap::inherit_sandbox_bind_fds(&mut probe, &plan);
                let output = crate::bounded_command::output_with_timeout(
                    probe,
                    std::time::Duration::from_secs(5),
                    crate::bounded_command::MAX_OUTPUT_BYTES,
                )
                .unwrap();
                assert!(output.status.success());
                assert_eq!(output.stdout, b"model: test\n");
            }
        }
    }
}

#[test]
fn extra_only_binds_reject_dangling_implicit_source_without_unwinding() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    std::fs::create_dir(home.join(".config")).unwrap();
    std::os::unix::fs::symlink(home.join("missing"), home.join(".config/gh-aider")).unwrap();
    let _home = ScopedEnvVar::set("HOME", &home);
    assert!(
        IsolationPlanBuilder::new(EnforcementMode::BestEffort)
            .with_filesystem_capability(FilesystemCapability::Bwrap)
            .build()
            .is_err()
    );
}

#[test]
fn test_builder_best_effort_with_bwrap() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let (_home, _env) = isolated_home(&temp);
    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_resource_capability(ResourceCapability::CgroupV2)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .build()
        .expect("BestEffort with Bwrap should succeed");

    assert_eq!(plan.resource, ResourceCapability::CgroupV2);
    assert_eq!(plan.filesystem, FilesystemCapability::Bwrap);
    assert!(plan.degraded_reasons.is_empty());
}

#[test]
fn non_bwrap_ordinary_readable_does_not_inherit_unused_descriptors() {
    use std::os::fd::AsRawFd;
    let temp = tempfile::tempdir().unwrap();
    for filesystem in [FilesystemCapability::None, FilesystemCapability::Landlock] {
        let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
            .with_filesystem_capability(filesystem)
            .with_readable_path(temp.path().to_path_buf())
            .build()
            .unwrap();
        let file = plan.readable_paths[0].pinned_source_file().unwrap();
        let mut command = std::process::Command::new("/bin/sh");
        command
            .args(["-c", "test ! -e \"$1\"", "probe"])
            .arg(format!("/proc/self/fd/{}", file.as_raw_fd()));
        crate::bwrap::try_inherit_sandbox_bind_fds(&mut command, &plan).unwrap();
        let output = crate::bounded_command::output_with_timeout(
            command,
            std::time::Duration::from_secs(5),
            crate::bounded_command::MAX_OUTPUT_BYTES,
        )
        .unwrap();
        assert!(
            output.status.success(),
            "{filesystem} must keep unused pins CLOEXEC"
        );
        assert_eq!(crate::bwrap::sandbox_bind_fd_count(&plan), 0);
    }
}

#[test]
fn public_bwrap_builder_checks_pinned_bind_capability() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (_home, _env) = isolated_home(&temp);
    let _path = install_legacy_bwrap(&temp);
    let file = root.join("file");
    let directory = root.join("directory");
    std::fs::write(&file, "accepted").unwrap();
    std::fs::create_dir(&directory).unwrap();
    for supported in [false, true] {
        if supported {
            std::fs::write(
                temp.path().join("bwrap"),
                "#!/bin/sh\necho '--ro-bind-fd FD DEST --bind-fd FD DEST'\n",
            )
            .unwrap();
        }
        for source in [&file, &directory] {
            for extra in [false, true] {
                let mut builder = crate::BwrapCommandBuilder::new("/bin/true", &[]);
                if extra {
                    builder.with_ro_bind(source, source);
                } else {
                    builder.with_readable_path(source);
                }
                let result = builder.build();
                if supported {
                    let command = result.expect("modern bwrap supports pinned binds");
                    assert!(command.get_args().any(|arg| arg == "--ro-bind-fd"));
                } else {
                    let error = result.expect_err("legacy bwrap must fail before command return");
                    assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
                }
            }
        }
        // Public plans can bypass plan-builder admission; command construction
        // must still enforce the same capability contract.
        let mut plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
            .with_filesystem_capability(FilesystemCapability::None)
            .with_readable_path(&file)
            .build()
            .unwrap();
        plan.filesystem = FilesystemCapability::Bwrap;
        let result = crate::from_isolation_plan(&plan, "/bin/true", &[]);
        if supported {
            assert!(result.unwrap().is_some());
        } else {
            assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Unsupported);
        }
    }
}

#[test]
fn public_bwrap_builder_preserves_no_bind_compatibility_and_path_errors() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (_home, _env) = isolated_home(&temp);
    let _path = install_legacy_bwrap(&temp);
    let file = root.join("readable");
    std::fs::write(&file, "accepted").unwrap();
    let socket = root.join("control.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let mut builder = crate::BwrapCommandBuilder::new("/bin/true", &[]);
    assert!(
        builder.build().is_ok(),
        "no binds need no new bwrap options"
    );
    builder.with_writable_path(&root).with_readable_path(&file);
    let command = builder.build().expect("covered readable emits no FD mount");
    assert!(!command.get_args().any(|arg| arg == "--ro-bind-fd"));
    builder.with_ro_bind(&socket, &socket);
    assert!(
        builder.build().is_ok(),
        "nonregular pathname binds remain supported"
    );
    builder.with_ro_bind(&root.join("missing"), &root.join("missing"));
    assert_eq!(
        builder.build().unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
}
