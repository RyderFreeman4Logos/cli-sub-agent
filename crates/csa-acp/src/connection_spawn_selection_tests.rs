//! Frozen ordinary-bind selection must survive ACP command reconstruction.

use super::*;
use std::os::unix::fs::{MetadataExt, symlink};

fn ordinary_bind_reconstruction(covered: bool, retarget: bool) {
    let temp = tempfile::tempdir_in("/var/tmp").unwrap();
    let root = temp.path().canonicalize().unwrap();
    // Outside /tmp and /dev: these mounts intentionally keep logical destinations.
    assert!(!root.starts_with("/tmp") && !root.starts_with("/dev"));
    let a = root.join("a");
    let b = root.join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let input = a.join("input");
    std::fs::write(&input, "accepted").unwrap();
    let accepted = std::fs::metadata(&input).unwrap();
    let alias = root.join("writable");
    symlink(if covered { &a } else { &b }, &alias).unwrap();
    let plan = csa_resource::isolation_plan::IsolationPlanBuilder::new(
        csa_resource::isolation_plan::EnforcementMode::BestEffort,
    )
    .with_resource_capability(ResourceCapability::Setrlimit)
    .with_filesystem_capability(FilesystemCapability::Bwrap)
    .with_writable_path(alias.clone())
    .with_readable_path(&input)
    .build()
    .unwrap();
    // Find the retained snapshot descriptor without opening another descriptor.
    let pin_fd = std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| {
            std::fs::metadata(entry.path())
                .is_ok_and(|meta| meta.dev() == accepted.dev() && meta.ino() == accepted.ino())
        })
        .unwrap()
        .file_name()
        .to_string_lossy()
        .into_owned();
    let env = HashMap::new();
    let prepared = AcpConnection::prepare_sandbox_command(
        AcpSpawnRequest {
            command: "/bin/true",
            args: &[],
            working_dir: &root,
            env: &env,
            options: AcpConnectionOptions::default(),
        },
        &AcpSandboxRequest {
            isolation_plan: &plan,
            tool_name: "codex",
            session_id: "01TEST",
            env_overrides: None,
        },
    )
    .unwrap();
    let emitted = prepared
        .effective_args
        .windows(3)
        .find(|args| args[0] == "--ro-bind-fd" && args[2] == input.to_string_lossy());
    assert_eq!(emitted.is_some(), !covered);
    if let Some(args) = emitted {
        assert_eq!(args[1], pin_fd);
    }
    if retarget {
        std::fs::remove_file(&alias).unwrap();
        symlink(if covered { &b } else { &a }, &alias).unwrap();
    }
    let mut probe = Command::new("/bin/sh");
    probe
        .args([
            "-c",
            if covered {
                "test ! -e /proc/self/fd/\"$1\""
            } else {
                "test /proc/self/fd/\"$1\" -ef \"$2\" && cat /proc/self/fd/\"$1\""
            },
            "probe",
            &pin_fd,
        ])
        .arg(&input);
    inherit_plan_bind_fds(&mut probe, &plan).unwrap();
    drop(plan);
    let output = csa_resource::bounded_command::output_with_timeout(
        probe.into_std(),
        Duration::from_secs(5),
        csa_resource::bounded_command::MAX_OUTPUT_BYTES,
    )
    .unwrap();
    assert!(
        output.status.success(),
        "covered={covered}, retarget={retarget}: {output:?}"
    );
    if !covered {
        assert_eq!(output.stdout, b"accepted");
    }
}

#[test]
fn temporal_selection_acp_emitted_fd_survives_alias_retarget() {
    ordinary_bind_reconstruction(false, true);
}

#[test]
fn temporal_selection_acp_unchanged_emitted_fd_is_inherited() {
    ordinary_bind_reconstruction(false, false);
}

#[test]
fn temporal_selection_acp_covered_snapshot_is_not_inherited() {
    ordinary_bind_reconstruction(true, false);
}

#[test]
fn temporal_selection_acp_inverse_retarget_does_not_inherit_unemitted_pin() {
    ordinary_bind_reconstruction(true, true);
}
