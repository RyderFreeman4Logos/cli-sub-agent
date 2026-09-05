//! Writable destinations are selected once, before descriptor admission.

use super::*;
use std::os::unix::fs::symlink;

fn admission_retarget(covered: bool, modern: bool, filesystem: FilesystemCapability) {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir_in("/var/tmp").unwrap();
    let (home, _env) = isolated_home(&temp);
    let root = home.canonicalize().unwrap();
    assert!(!root.starts_with("/tmp") && !root.starts_with("/dev"));
    let _path = install_legacy_bwrap(&temp);
    if modern {
        install_modern_bwrap(&temp);
    }
    let a = root.join("a");
    let b = root.join("b");
    let project = root.join("project");
    let session = root.join("session");
    for dir in [&a, &b, &project, &session] {
        std::fs::create_dir(dir).unwrap();
    }
    let input = a.join("input");
    std::fs::write(&input, "accepted").unwrap();
    let alias = root.join("state");
    symlink(if covered { &a } else { &b }, &alias).unwrap();
    let _xdg = ScopedEnvVar::set("XDG_STATE_HOME", &alias);
    let builder = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(filesystem)
        .with_resource_capability(ResourceCapability::CgroupV2)
        .with_tool_defaults("test-tool", &project, &session)
        .with_readable_path(&input);
    assert!(
        builder.writable_paths.contains(&alias),
        "real XDG default retains alias until admission"
    );
    let plan = builder.build().unwrap();
    let expected = usize::from(!covered && filesystem == FilesystemCapability::Bwrap);
    assert_eq!(crate::bwrap::sandbox_bind_fd_count(&plan), expected);
    let expected_resource = if expected > 0 || filesystem == FilesystemCapability::Landlock {
        ResourceCapability::Setrlimit
    } else {
        ResourceCapability::CgroupV2
    };
    assert_eq!(plan.resource, expected_resource);
    std::fs::remove_file(&alias).unwrap();
    symlink(if covered { &b } else { &a }, &alias).unwrap();
    let built = crate::from_isolation_plan(&plan, "/bin/true", &[])
        .expect("admitted mount selection must not change after alias retarget");
    if let Some(built) = built {
        assert_eq!(emitted_fds(&built).len(), expected);
        let args: Vec<_> = built.get_args().collect();
        let destination = if covered { &a } else { &b };
        assert!(
            args.windows(3)
                .any(|args| args[0] == "--bind" && args[2] == destination)
        );
    } else {
        assert_ne!(filesystem, FilesystemCapability::Bwrap);
    }
    assert_eq!(crate::bwrap::sandbox_bind_fd_count(&plan), expected);
    assert_eq!(plan.resource, expected_resource);
    let mut systemd = Command::new("systemd-run");
    let result = crate::bwrap::try_inherit_sandbox_bind_fds(&mut systemd, &plan);
    if expected == 0 {
        result.unwrap();
    } else {
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
    }
}

#[test]
fn temporal_selection_added_writable_alias_keeps_its_destination() {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir_in("/var/tmp").unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (_home, _env) = isolated_home(&temp);
    let _path = install_legacy_bwrap(&temp);
    let a = root.join("a");
    let b = root.join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let alias = root.join("runtime-home");
    symlink(&a, &alias).unwrap();
    let mut plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .build()
        .unwrap();
    assert!(plan.add_writable_dir_or_creatable_parent(&alias));
    std::fs::remove_file(&alias).unwrap();
    symlink(&b, &alias).unwrap();
    let command = crate::from_isolation_plan(&plan, "/bin/true", &[])
        .unwrap()
        .unwrap();
    let args: Vec<_> = command.get_args().collect();
    assert!(
        args.windows(3)
            .any(|args| args[0] == "--bind" && args[1] == a && args[2] == a)
    );
}

#[test]
fn temporal_selection_admission_covered_legacy_alias_retarget() {
    admission_retarget(true, false, FilesystemCapability::Bwrap);
}

#[test]
fn temporal_selection_admission_uncovered_modern_alias_retarget() {
    admission_retarget(false, true, FilesystemCapability::Bwrap);
}

#[test]
fn temporal_selection_admission_non_bwrap_alias_retarget() {
    for filesystem in [FilesystemCapability::None, FilesystemCapability::Landlock] {
        admission_retarget(false, false, filesystem);
    }
}

fn public_collection_retarget(covered: bool) {
    let _lock = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir_in("/var/tmp").unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (_home, _env) = isolated_home(&temp);
    let _path = install_legacy_bwrap(&temp);
    let a = root.join("a");
    let b = root.join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let input = a.join("input");
    std::fs::write(&input, "accepted").unwrap();
    let alias = root.join("state");
    symlink(if covered { &a } else { &b }, &alias).unwrap();
    let readable = ReadablePath::from(&input);
    let pin = readable.pinned_source_file().unwrap();
    let fd = pin.as_raw_fd().to_string();
    // Execute the returned Command, including its real ownership/pre_exec closure.
    std::fs::write(temp.path().join("bwrap"), format!(
        "#!/bin/sh\n[ \"$1\" = --help ] && {{ echo '--ro-bind-fd FD DEST --bind-fd FD DEST'; exit 0; }}\n{}\n",
        if covered {
            format!("test ! -e /proc/self/fd/{fd}")
        } else {
            format!("test /proc/self/fd/{fd} -ef '{}' && cat /proc/self/fd/{fd}", input.display())
        }
    )).unwrap();
    let mut builder = crate::BwrapCommandBuilder::new("/bin/true", &[]);
    builder
        .with_writable_path(&alias)
        .with_readable_path(readable);
    crate::bwrap::AFTER_BIND_FILE_COLLECTION.set(Some(Box::new(move || {
        std::fs::remove_file(&alias).unwrap();
        symlink(if covered { &b } else { &a }, &alias).unwrap();
    })));
    let command = builder.build().unwrap();
    assert_eq!(
        emitted_fds(&command),
        if covered { vec![] } else { vec![fd] }
    );
    drop(builder);
    let output = crate::bounded_command::output_with_timeout(
        command,
        std::time::Duration::from_secs(5),
        crate::bounded_command::MAX_OUTPUT_BYTES,
    )
    .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        output.stdout,
        if covered {
            b"".as_slice()
        } else {
            b"accepted".as_slice()
        }
    );
}

#[test]
fn temporal_selection_public_collection_emission_uncovered_retarget() {
    public_collection_retarget(false);
}

#[test]
fn temporal_selection_public_collection_emission_covered_retarget() {
    public_collection_retarget(true);
}
