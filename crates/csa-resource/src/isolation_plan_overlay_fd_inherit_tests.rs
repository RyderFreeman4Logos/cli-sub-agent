//! ACP/cgroup argv reconstruction must keep overlay bind FDs (#3148).

use super::*;
use crate::sandbox::ResourceCapability;
use std::path::Path;

fn overlay_bind_plan(overlay: ReadablePath) -> IsolationPlan {
    IsolationPlan {
        resource: ResourceCapability::None,
        filesystem: FilesystemCapability::Bwrap,
        writable_paths: Vec::new(),
        readable_paths: vec![overlay],
        env_overrides: std::collections::HashMap::new(),
        degraded_reasons: Vec::new(),
        memory_max_mb: None,
        memory_swap_max_mb: None,
        pids_max: None,
        readonly_project_root: false,
        project_root: None,
        soft_limit_percent: None,
        memory_monitor_interval_seconds: None,
        user_daemon_ipc: false,
    }
}

fn reconstruct_like_acp(command: &std::process::Command) -> std::process::Command {
    let mut reconstructed = std::process::Command::new(command.get_program());
    reconstructed.args(command.get_args());
    reconstructed
}

#[cfg(all(unix, target_os = "linux"))]
#[test]
fn acp_style_reconstruction_keeps_overlay_ro_bind_fd_usable() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("overlay-fd-inherit-")
        .tempdir_in("/var/tmp")
        .expect("tempdir outside /tmp overlay");
    let dest = temp.path().join("config.yaml");
    std::fs::write(&dest, "pinned-overlay\n").expect("write overlay leaf");
    let overlay = ReadablePath::try_pinned_readonly_overlay(dest.clone())
        .expect("regular overlay leaf must pin");
    let plan = overlay_bind_plan(overlay);
    let built = crate::from_isolation_plan(&plan, "/bin/cat", &[dest.to_string_lossy().into()])
        .expect("pinned overlay must produce a bwrap command");
    let args: Vec<String> = built
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert!(
        args.windows(3)
            .any(|window| window[0] == "--ro-bind-fd" && window[2] == dest.to_string_lossy()),
        "overlay must use --ro-bind-fd; args: {args:?}"
    );

    let mut reconstructed = reconstruct_like_acp(&built);
    drop(built);
    crate::bwrap::inherit_sandbox_bind_fds(&mut reconstructed, &plan);
    reconstructed.env_clear();
    reconstructed.env("PATH", "/usr/bin:/bin");
    let output = reconstructed
        .output()
        .expect("reconstructed bwrap command must spawn");
    assert!(
        output.status.success(),
        "ACP argv reconstruction must keep overlay --ro-bind-fd usable; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"pinned-overlay\n");
}

#[cfg(all(unix, target_os = "linux"))]
#[test]
fn cgroup_style_reconstruction_fail_closed_or_keeps_overlay_ro_bind_fd() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("overlay-fd-cgroup-")
        .tempdir_in("/var/tmp")
        .expect("tempdir outside /tmp overlay");
    let dest = temp.path().join("config.yaml");
    std::fs::write(&dest, "pinned-overlay\n").expect("write overlay leaf");
    let overlay = ReadablePath::try_pinned_readonly_overlay(dest.clone())
        .expect("regular overlay leaf must pin");
    let mut plan = overlay_bind_plan(overlay);
    plan.resource = ResourceCapability::CgroupV2;

    let built = crate::from_isolation_plan(&plan, "/bin/cat", &[dest.to_string_lossy().into()])
        .expect("pinned overlay must produce a bwrap command");
    let mut reconstructed = std::process::Command::new("systemd-run");
    reconstructed.arg("--user");
    reconstructed.arg("--scope");
    reconstructed.arg("--");
    reconstructed.arg(built.get_program());
    reconstructed.args(built.get_args());
    drop(built);

    match crate::bwrap::try_inherit_sandbox_bind_fds(&mut reconstructed, &plan) {
        Ok(()) => {
            reconstructed.env_clear();
            reconstructed.env("PATH", "/usr/bin:/bin");
            let output = reconstructed
                .output()
                .expect("cgroup reconstruction with inherited FDs must spawn");
            assert!(
                output.status.success(),
                "cgroup reconstruction must keep overlay --ro-bind-fd usable; stderr={}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(output.stdout, b"pinned-overlay\n");
        }
        Err(error) => {
            let message = error.to_string();
            assert!(
                message.contains("bind-fd") || message.contains("ro-bind-fd"),
                "cgroup reconstruction that cannot pass overlay FDs must fail closed: {message}"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn hermes_cgroup_plan_does_not_drop_overlay_bind_fds() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("hermes-cgroup-fd-")
        .tempdir_in("/var/tmp")
        .expect("tempdir outside /tmp overlay");
    let (_home, _env) = isolated_home(&temp);
    let hermes_home = temp.path().join("hermes-home");
    std::fs::create_dir_all(hermes_home.join("logs")).expect("create logs");
    std::fs::write(hermes_home.join("config.yaml"), "model: test\n").expect("write config");
    let execution_env = std::collections::HashMap::from([(
        "HERMES_HOME".to_string(),
        hermes_home.to_string_lossy().into_owned(),
    )]);
    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_resource_capability(ResourceCapability::CgroupV2)
        .with_execution_env(Some(&execution_env))
        .with_tool_defaults(
            "hermes",
            Path::new("/tmp/project"),
            Path::new("/tmp/session"),
        )
        .build()
        .expect("Hermes overlay plan must build");
    assert!(
        plan.resource != ResourceCapability::CgroupV2
            || crate::bwrap::sandbox_bind_fd_count(&plan) == 0,
        "CgroupV2 reconstruction cannot keep overlay FDs; degrade or omit bind-fd"
    );
    if plan.resource != ResourceCapability::CgroupV2 {
        assert!(
            crate::bwrap::sandbox_bind_fd_count(&plan) > 0,
            "degraded Hermes plan must still pin overlay bind FDs"
        );
    }
}
