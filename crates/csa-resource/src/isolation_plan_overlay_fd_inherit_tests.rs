//! ACP/cgroup argv reconstruction must keep overlay bind FDs (#3148).

use super::*;
use crate::sandbox::ResourceCapability;
use std::path::Path;

use crate::bounded_command::{output_with_timeout, status_with_timeout};

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

fn overlay_ro_bind_fd(args: &[String], dest: &Path) -> i32 {
    let dest = dest.to_string_lossy();
    args.windows(3)
        .find(|window| window[0] == "--ro-bind-fd" && window[2] == dest.as_ref())
        .and_then(|window| window[1].parse().ok())
        .unwrap_or_else(|| panic!("overlay must use --ro-bind-fd; args: {args:?}"))
}

/// Prove `--ro-bind-fd N` stays open across exec without nested bwrap unshare.
#[cfg(all(unix, target_os = "linux"))]
fn assert_overlay_bind_fd_survives_exec(plan: &IsolationPlan, fd: i32, expected: &[u8]) {
    assert!(
        Path::new(&format!("/proc/self/fd/{fd}")).exists(),
        "overlay --ro-bind-fd {fd} must stay open after reconstruction"
    );
    let mut probe = std::process::Command::new("/bin/cat");
    probe.arg(format!("/proc/self/fd/{fd}"));
    crate::bwrap::inherit_sandbox_bind_fds(&mut probe, plan);
    probe.env_clear();
    probe.env("PATH", "/usr/bin:/bin");
    let output = output_with_timeout(probe, std::time::Duration::from_secs(5))
        .expect("overlay FD exec probe must complete within its bound");
    assert!(
        output.status.success(),
        "overlay --ro-bind-fd {fd} must survive exec without nested bwrap; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, expected);
}

#[cfg(all(unix, target_os = "linux"))]
#[test]
fn overlay_fd_exec_probe_uses_bounded_subprocess_helper() {
    let source = include_str!("isolation_plan_overlay_fd_inherit_tests.rs");
    let unbounded = [".out", "put()"].concat();
    assert!(
        !source.contains(&unbounded),
        "overlay FD exec probe must not use unbounded Command output"
    );
    assert!(
        source.contains("output_with_timeout"),
        "overlay FD exec probe must use the repository bounded subprocess helper"
    );
}

#[cfg(all(unix, target_os = "linux"))]
#[test]
fn overlay_fd_exec_probe_timeout_cleans_up_process_group() {
    let mut probe = std::process::Command::new("/bin/sleep");
    probe.arg("30");
    probe.env_clear();
    probe.env("PATH", "/usr/bin:/bin");
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        status_with_timeout(probe, std::time::Duration::from_millis(200))
            .expect("bounded overlay FD probe must complete within its bound");
    }));
    assert!(
        panicked.is_err(),
        "bounded overlay FD probe must time out instead of hanging"
    );
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
    let fd = overlay_ro_bind_fd(&args, &dest);

    let mut reconstructed = reconstruct_like_acp(&built);
    drop(built);
    crate::bwrap::inherit_sandbox_bind_fds(&mut reconstructed, &plan);
    assert_overlay_bind_fd_survives_exec(&plan, fd, b"pinned-overlay\n");
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
    let args: Vec<String> = built
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let fd = overlay_ro_bind_fd(&args, &dest);
    let mut reconstructed = std::process::Command::new("systemd-run");
    reconstructed.arg("--user");
    reconstructed.arg("--scope");
    reconstructed.arg("--");
    reconstructed.arg(built.get_program());
    reconstructed.args(built.get_args());
    drop(built);

    match crate::bwrap::try_inherit_sandbox_bind_fds(&mut reconstructed, &plan) {
        Ok(()) => {
            assert_overlay_bind_fd_survives_exec(&plan, fd, b"pinned-overlay\n");
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
