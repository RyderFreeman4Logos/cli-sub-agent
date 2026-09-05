//! Emitted binds, not retained snapshot pins, determine FD admission (#3174).

use super::*;
use std::os::fd::AsRawFd;
use std::process::Command;

fn install_modern_bwrap(temp: &tempfile::TempDir) {
    std::fs::write(
        temp.path().join("bwrap"),
        "#!/bin/sh\necho '--ro-bind-fd FD DEST --bind-fd FD DEST'\n",
    )
    .unwrap();
}

fn emitted_fds(command: &Command) -> Vec<String> {
    let args: Vec<_> = command.get_args().collect();
    args.windows(3)
        .filter(|args| args[0] == "--ro-bind-fd" || args[0] == "--bind-fd")
        .map(|args| args[1].to_string_lossy().into_owned())
        .collect()
}

fn covered_readable_plan(supported: bool) {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (_home, _env) = isolated_home(&temp);
    let _path = install_legacy_bwrap(&temp);
    if supported {
        install_modern_bwrap(&temp);
    }
    let file = root.join("input");
    std::fs::write(&file, "accepted").unwrap();
    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_resource_capability(ResourceCapability::CgroupV2)
        .with_writable_path(root.clone())
        .with_readable_path(&file)
        .build()
        .expect("covered readable needs no descriptor-bind capability");
    assert_eq!(plan.resource, ResourceCapability::CgroupV2);
    assert!(plan.degraded_reasons.is_empty());
    assert_eq!(crate::bwrap::sandbox_bind_fd_count(&plan), 0);
    let built = crate::from_isolation_plan(&plan, "/bin/true", &[])
        .unwrap()
        .unwrap();
    assert!(emitted_fds(&built).is_empty());
    let mut public = crate::BwrapCommandBuilder::new("/bin/true", &[]);
    public.with_writable_path(&root).with_readable_path(&file);
    assert!(emitted_fds(&public.build().unwrap()).is_empty());
    let mut systemd = Command::new("systemd-run");
    crate::bwrap::try_inherit_sandbox_bind_fds(&mut systemd, &plan).unwrap();

    // The snapshot remains owned, but reconstructed commands must not inherit it.
    let pin = plan.readable_paths[0].pinned_source_file().unwrap();
    let mut probe = Command::new("/bin/sh");
    probe
        .args(["-c", "test ! -e \"$1\"", "probe"])
        .arg(format!("/proc/self/fd/{}", pin.as_raw_fd()));
    crate::bwrap::try_inherit_sandbox_bind_fds(&mut probe, &plan).unwrap();
    drop(built);
    drop(plan);
    assert!(pin.metadata().unwrap().is_file());
    let output = crate::bounded_command::output_with_timeout(
        probe,
        std::time::Duration::from_secs(5),
        crate::bounded_command::MAX_OUTPUT_BYTES,
    )
    .unwrap();
    assert!(output.status.success());
}

#[test]
fn covered_readable_legacy_plan_preserves_cgroup_without_bind_fds() {
    covered_readable_plan(false);
}

#[test]
fn covered_readable_modern_plan_preserves_cgroup_without_bind_fds() {
    covered_readable_plan(true);
}

#[test]
fn emitted_bind_projection_matches_plan_and_public_builder_combinations() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (_home, _env) = isolated_home(&temp);
    let _path = install_legacy_bwrap(&temp);
    let project = root.join("project");
    let nested = project.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let child = nested.join("input");
    let outside = root.join("outside");
    std::fs::write(&child, "child").unwrap();
    std::fs::write(&outside, "outside").unwrap();

    for supported in [false, true] {
        if supported {
            install_modern_bwrap(&temp);
        }
        for filesystem in [
            FilesystemCapability::Bwrap,
            FilesystemCapability::None,
            FilesystemCapability::Landlock,
        ] {
            for case in [
                "covered",
                "uncovered",
                "extra",
                "overlay",
                "nested",
                "readonly-project",
            ] {
                let readable = match case {
                    "uncovered" => ReadablePath::from(&outside),
                    "extra" => ReadablePath::pinned_extra(child.clone(), child.clone()),
                    "overlay" | "nested" => {
                        ReadablePath::try_pinned_readonly_overlay(nested.clone()).unwrap()
                    }
                    _ => ReadablePath::from(&child),
                };
                let mut builder = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
                    .with_filesystem_capability(filesystem)
                    .with_resource_capability(ResourceCapability::CgroupV2)
                    .with_writable_path(project.clone())
                    .with_project_root(&project)
                    .with_readonly_project_root(case == "readonly-project")
                    .with_readable_path(readable.clone());
                let mut public = crate::BwrapCommandBuilder::new("/bin/true", &[]);
                if case == "readonly-project" {
                    public.with_ro_bind(&project, &project);
                } else {
                    public.with_writable_path(&project);
                }
                if case == "extra" {
                    public.with_ro_bind(&child, &child);
                } else {
                    public.with_readable_path(readable);
                }
                if case == "nested" {
                    let writable = ReadablePath::pinned_writable(
                        child.clone(),
                        std::fs::File::open(&child).unwrap(),
                    );
                    builder = builder
                        .with_writable_path(child.clone())
                        .with_readable_path(writable.clone());
                    public
                        .with_writable_path(&child)
                        .with_readable_path(writable);
                }
                let expected = match case {
                    "covered" => 0,
                    "nested" | "readonly-project" => 2,
                    _ => 1,
                };
                let result = builder.build();
                let public_result = public.build();
                if !supported && expected > 0 {
                    assert_eq!(
                        public_result.unwrap_err().kind(),
                        std::io::ErrorKind::Unsupported
                    );
                    if filesystem == FilesystemCapability::Bwrap {
                        let error = result.unwrap_err();
                        assert!(error.to_string().contains("bind-fd"), "{case}: {error:#}");
                        continue;
                    }
                } else {
                    assert_eq!(
                        emitted_fds(&public_result.unwrap()).len(),
                        expected,
                        "{case}"
                    );
                }
                let plan = result.unwrap();
                let built = crate::from_isolation_plan(&plan, "/bin/true", &[]).unwrap();
                let fds = if filesystem == FilesystemCapability::Bwrap {
                    let built = built.unwrap();
                    let fds = emitted_fds(&built);
                    assert_eq!(fds.len(), expected, "{case}");
                    fds
                } else {
                    assert!(built.is_none());
                    Vec::new()
                };
                let count = fds.len();
                assert_eq!(crate::bwrap::sandbox_bind_fd_count(&plan), count, "{case}");
                let resource = if filesystem == FilesystemCapability::Landlock || count > 0 {
                    ResourceCapability::Setrlimit
                } else {
                    ResourceCapability::CgroupV2
                };
                assert_eq!(plan.resource, resource, "{filesystem}: {case}");
                let mut systemd = Command::new("systemd-run");
                assert_eq!(
                    crate::bwrap::try_inherit_sandbox_bind_fds(&mut systemd, &plan).is_ok(),
                    count == 0
                );
                let pins: Vec<_> = plan
                    .readable_paths
                    .iter()
                    .filter_map(ReadablePath::pinned_source_file)
                    .collect();
                let mut probe = Command::new("/bin/sh");
                probe.args(["-c", "while [ $# -gt 0 ]; do if [ \"$2\" = yes ]; then test -e /proc/self/fd/\"$1\" || exit 1; else test ! -e /proc/self/fd/\"$1\" || exit 2; fi; shift 2; done", "probe"]);
                for pin in &pins {
                    let fd = pin.as_raw_fd().to_string();
                    probe.args([fd.as_str(), if fds.contains(&fd) { "yes" } else { "no" }]);
                }
                crate::bwrap::try_inherit_sandbox_bind_fds(&mut probe, &plan).unwrap();
                drop(plan);
                let output = crate::bounded_command::output_with_timeout(
                    probe,
                    std::time::Duration::from_secs(5),
                    crate::bounded_command::MAX_OUTPUT_BYTES,
                )
                .unwrap();
                assert!(output.status.success(), "{filesystem}: {case}");
            }
        }
    }
}
