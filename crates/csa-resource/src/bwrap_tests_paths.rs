use super::*;

fn ro_bind_destination(args: &[String], destination: &str) -> Option<usize> {
    args.windows(3).position(|window| {
        (window[0] == "--ro-bind" && window[2] == destination)
            || (window[0] == "--ro-bind-fd" && window[2] == destination)
    })
}

#[test]
fn test_bwrap_command_with_readable_tmp_file() {
    let temp = tempfile::tempdir_in("/tmp").expect("/tmp fixture");
    let readable = temp.path().join("foo.json");
    std::fs::write(&readable, "{}").expect("write readable file");

    let mut builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
    builder.with_readable_path(&readable);
    let cmd = builder.build().expect("valid bind paths");
    let args = command_args(&cmd);
    let readable_str = readable.to_string_lossy().into_owned();

    let tmpfs_pos = args
        .iter()
        .position(|arg| arg == "--tmpfs")
        .expect("--tmpfs must be present");
    let ro_bind_pos = ro_bind_destination(&args, &readable_str)
        .expect("read-only FD bind readable path must be present");

    assert_eq!(args[tmpfs_pos + 1], "/tmp");
    assert!(
        tmpfs_pos < ro_bind_pos,
        "readable --ro-bind must come after --tmpfs /tmp; args: {args:?}"
    );
}

#[test]
fn test_bwrap_readable_tmp_root_rejected() {
    let mut builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
    builder.with_readable_path(Path::new("/tmp"));
    assert!(
        builder
            .build_with_home(None)
            .unwrap_err()
            .to_string()
            .contains("must not be /tmp itself")
    );
}

#[test]
fn test_bwrap_readable_and_writable_paths_after_tmpfs() {
    let temp = tempfile::tempdir_in("/tmp").expect("/tmp fixture");
    let readable = temp.path().join("bar.txt");
    std::fs::write(&readable, "hello").expect("write readable file");

    let mut builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
    builder.with_writable_path(Path::new("/tmp/work"));
    builder.with_readable_path(&readable);
    let cmd = builder.build().expect("valid bind paths");
    let args = command_args(&cmd);
    let readable_str = readable.to_string_lossy().into_owned();

    let tmpfs_pos = args
        .iter()
        .position(|arg| arg == "--tmpfs")
        .expect("--tmpfs must be present");
    let writable_bind_pos = args
        .windows(3)
        .position(|window| window[0] == "--bind" && window[1] == "/tmp/work")
        .expect("writable bind should be present");
    let readable_bind_pos = ro_bind_destination(&args, &readable_str)
        .expect("read-only FD bind readable path must be present");

    assert!(
        tmpfs_pos < writable_bind_pos,
        "writable bind must come after tmpfs; args: {args:?}"
    );
    assert!(
        tmpfs_pos < readable_bind_pos,
        "readable bind must come after tmpfs; args: {args:?}"
    );
}

#[test]
fn test_bwrap_duplicate_readable_writable_path_keeps_writable_bind() {
    let temp = tempfile::tempdir_in("/tmp").expect("/tmp fixture");
    let path = temp.path();
    let path_str = path.to_string_lossy().into_owned();

    let mut builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
    builder.with_writable_path(path);
    builder.with_readable_path(path);
    let cmd = builder.build().expect("valid bind paths");
    let args = command_args(&cmd);

    assert!(
        args.windows(3)
            .any(|window| window[0] == "--bind" && window[1] == path_str && window[2] == path_str),
        "duplicate readable+writable path must remain writable; args: {args:?}"
    );
    assert!(
        !args.windows(3).any(|window| {
            window[0] == "--ro-bind" && window[1] == path_str && window[2] == path_str
        }),
        "duplicate readable+writable path must not be remounted read-only; args: {args:?}"
    );
}

#[test]
fn test_bwrap_nested_tmp_path_creates_intermediate_dirs() {
    let mut builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
    builder.with_writable_path(Path::new("/tmp/deep/nested/dir"));
    let cmd = builder.build().expect("valid bind paths");
    let args = command_args(&cmd);

    // Must have --dir for intermediate parent
    let has_parent_dir = args
        .iter()
        .enumerate()
        .filter(|(_, a)| *a == "--dir")
        .any(|(i, _)| args.get(i + 1).map(|s| s.as_str()) == Some("/tmp/deep/nested"));
    assert!(
        has_parent_dir,
        "nested /tmp path must have --dir for parent /tmp/deep/nested; args: {args:?}"
    );

    // Must have --dir for the path itself
    let has_path_dir = args
        .iter()
        .enumerate()
        .filter(|(_, a)| *a == "--dir")
        .any(|(i, _)| args.get(i + 1).map(|s| s.as_str()) == Some("/tmp/deep/nested/dir"));
    assert!(
        has_path_dir,
        "nested /tmp path must have --dir for /tmp/deep/nested/dir; args: {args:?}"
    );

    // Must have --bind
    let has_bind = args
        .iter()
        .enumerate()
        .filter(|(_, a)| *a == "--bind")
        .any(|(i, _)| args.get(i + 1).map(|s| s.as_str()) == Some("/tmp/deep/nested/dir"));
    assert!(
        has_bind,
        "/tmp/deep/nested/dir must have --bind; args: {args:?}"
    );
}

#[test]
fn test_bwrap_bare_tmp_is_bind_mounted_when_explicitly_writable() {
    // /tmp is an explicit config grant, not a request for empty tmpfs.
    let mut builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
    builder.with_writable_path(Path::new("/tmp"));
    let cmd = builder.build().expect("valid bind paths");
    let args = command_args(&cmd);

    // --tmpfs /tmp must exist
    assert!(
        args.windows(2).any(|w| w[0] == "--tmpfs" && w[1] == "/tmp"),
        "--tmpfs /tmp must be present; args: {args:?}"
    );

    let tmpfs_pos = args
        .windows(2)
        .position(|w| w[0] == "--tmpfs" && w[1] == "/tmp")
        .expect("--tmpfs /tmp must be present");
    let bind_tmp_pos = args
        .windows(3)
        .position(|w| w[0] == "--bind" && w[1] == "/tmp" && w[2] == "/tmp")
        .expect("--bind /tmp /tmp must be present");
    assert!(
        tmpfs_pos < bind_tmp_pos,
        "--bind /tmp /tmp must come after --tmpfs /tmp; args: {args:?}"
    );
}

#[test]
fn test_bwrap_auto_ro_binds_gh_aider_config_when_present() {
    let home = tempfile::tempdir_in("/tmp").expect("/tmp fixture");
    let gh_aider = home.path().join(".config/gh-aider");
    std::fs::create_dir_all(&gh_aider).expect("create gh-aider dir");

    let builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
    let cmd = builder
        .build_with_home(Some(home.path()))
        .expect("valid bind paths");
    let args = command_args(&cmd);

    assert!(
        ro_bind_destination(&args, &gh_aider.to_string_lossy()).is_some(),
        "~/.config/gh-aider should be explicitly re-bound read-only so sandboxed gh commands can still read the aider auth config; args: {args:?}"
    );
}

#[cfg(unix)]
#[test]
fn from_isolation_plan_keeps_tmp_symlink_logical_readable_destination() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir_in("/tmp").expect("/tmp fixture");
    let canonical_source = temp.path().join("canonical-source.json");
    let logical_destination = temp.path().join("logical-readable.json");
    std::fs::write(&canonical_source, "{}").expect("write canonical source");
    symlink(&canonical_source, &logical_destination).expect("create logical symlink");

    let readable_paths = crate::isolation_plan::validate_readable_paths(
        std::slice::from_ref(&logical_destination),
        temp.path(),
    )
    .expect("/tmp symlink should be accepted after canonical source validation");
    assert_eq!(
        readable_paths
            .iter()
            .map(|path| path.requested().to_path_buf())
            .collect::<Vec<_>>(),
        vec![logical_destination.clone()]
    );

    let plan = IsolationPlan {
        resource: ResourceCapability::None,
        filesystem: FilesystemCapability::Bwrap,
        writable_paths: Vec::new(),
        readable_paths,
        env_overrides: HashMap::new(),
        degraded_reasons: Vec::new(),
        memory_max_mb: None,
        memory_swap_max_mb: None,
        pids_max: None,
        readonly_project_root: false,
        project_root: None,
        soft_limit_percent: None,
        memory_monitor_interval_seconds: None,
        user_daemon_ipc: false,
    };
    let args = command_args(
        &from_isolation_plan(&plan, "/usr/bin/tool", &[])
            .expect("valid bind paths")
            .expect("bwrap isolation plan"),
    );

    assert!(
        ro_bind_destination(&args, &logical_destination.to_string_lossy()).is_some(),
        "readable bind must use the canonical source at the /tmp logical destination; args: {args:?}"
    );
}

#[cfg(unix)]
#[test]
fn from_isolation_plan_does_not_remount_writable_canonical_child_readonly_through_alias() {
    use std::os::unix::fs::symlink;

    let base = tempfile::Builder::new()
        .prefix("bwrap-3101-")
        .tempdir_in("/var/tmp")
        .expect("tempdir");
    let writable_tree = base.path().join("project");
    let canonical_file = writable_tree.join("file");
    std::fs::create_dir_all(&writable_tree).expect("create writable tree");
    std::fs::write(&canonical_file, "{}").expect("write canonical file");

    let alias_root = base.path().join("read-alias");
    symlink(&writable_tree, &alias_root).expect("create readable alias");
    let readable_alias = alias_root.join("file");

    let plan = IsolationPlan {
        resource: ResourceCapability::None,
        filesystem: FilesystemCapability::Bwrap,
        writable_paths: vec![writable_tree],
        readable_paths: vec![readable_alias.into()],
        env_overrides: HashMap::new(),
        degraded_reasons: Vec::new(),
        memory_max_mb: None,
        memory_swap_max_mb: None,
        pids_max: None,
        readonly_project_root: false,
        project_root: None,
        soft_limit_percent: None,
        memory_monitor_interval_seconds: None,
        user_daemon_ipc: false,
    };
    let args = command_args(
        &from_isolation_plan(&plan, "/usr/bin/tool", &[])
            .expect("valid bind paths")
            .expect("bwrap isolation plan"),
    );
    let canonical = canonical_file.to_string_lossy();

    assert!(
        !args.windows(3).any(|window| {
            window[0] == "--ro-bind" && window[1] == canonical && window[2] == canonical
        }),
        "writable canonical child must not be remounted read-only through an alias; args: {args:?}"
    );
}

#[test]
fn test_bwrap_readable_path_binds_resolved_dest_when_logical_parents_missing() {
    // Repro for #3075: on autofs/logical worktrees the logical path's parent
    // directories do not exist on the host root, so a --ro-bind whose dest is
    // the logical path makes bwrap fail with "Can't mkdir parents". The bind
    // must target the resolved (real) path just like writable binds do.
    //
    // The real/logical trees must NOT live under /tmp: /tmp is a fresh tmpfs
    // in the sandbox, where the logical destination is correct (the resolved
    // path is hidden behind the overlay). The bug is specific to non-/tmp
    // paths such as autofs-backed HOME worktrees.
    let base = tempfile::Builder::new()
        .prefix("bwrap-3075-")
        .tempdir_in("/var/tmp")
        .expect("tempdir");
    let real_file = base
        .path()
        .join("real")
        .join("ctx")
        .join("review-context.md");
    std::fs::create_dir_all(real_file.parent().expect("parent")).expect("create real parent");
    std::fs::write(&real_file, "{}").expect("write readable file");

    // Logical path whose ".csa" parent exists only as a symlink into `real`.
    let logical_parent = base
        .path()
        .join("logical")
        .join("worktrees")
        .join("49-config-toml")
        .join(".csa");
    std::fs::create_dir_all(logical_parent.parent().expect("parent"))
        .expect("create logical parents");
    std::os::unix::fs::symlink(base.path().join("real").join("ctx"), &logical_parent)
        .expect("symlink .csa -> real/ctx");
    let logical_file = logical_parent.join("review-context.md");
    std::fs::write(&logical_file, "{}").expect("write via logical path");

    let resolved = logical_file.canonicalize().expect("canonicalize");
    assert_ne!(
        resolved, logical_file,
        "test setup must produce a logical path distinct from its resolved path"
    );

    let mut builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
    builder.with_readable_path(&logical_file);
    let cmd = builder.build().expect("valid bind paths");
    let args = command_args(&cmd);

    let resolved_str = resolved.to_string_lossy();
    let logical_str = logical_file.to_string_lossy();
    assert!(
        ro_bind_destination(&args, &resolved_str).is_some(),
        "readable bind must use the resolved destination so bwrap never sees the missing logical parents; args: {args:?}"
    );
    assert!(
        ro_bind_destination(&args, &logical_str).is_none(),
        "readable bind must not target the logical dest; args: {args:?}"
    );
}

#[cfg(unix)]
#[test]
fn from_isolation_plan_pins_validated_readable_symlink_bind_source() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir_in("/tmp").expect("/tmp fixture");
    let validated_target = temp.path().join("validated-target.json");
    let replaced_target = temp.path().join("replaced-target.json");
    let readable_symlink = temp.path().join("readable-link.json");
    std::fs::write(&validated_target, "validated").expect("write validated target");
    std::fs::write(&replaced_target, "replaced").expect("write replaced target");
    symlink(&validated_target, &readable_symlink).expect("create readable symlink");

    let readable_paths = crate::isolation_plan::validate_readable_paths(
        std::slice::from_ref(&readable_symlink),
        temp.path(),
    )
    .expect("readable symlink should validate against its canonical target");
    assert_eq!(
        readable_paths
            .iter()
            .map(|path| path.requested().to_path_buf())
            .collect::<Vec<_>>(),
        vec![readable_symlink.clone()]
    );
    assert_eq!(
        readable_paths[0].bind_source(),
        validated_target
            .canonicalize()
            .expect("canonical validated target")
            .as_path()
    );

    std::fs::remove_file(&readable_symlink).expect("remove symlink for replacement");
    symlink(&replaced_target, &readable_symlink).expect("replace readable symlink");

    let plan = IsolationPlan {
        resource: ResourceCapability::None,
        filesystem: FilesystemCapability::Bwrap,
        writable_paths: Vec::new(),
        readable_paths,
        env_overrides: HashMap::new(),
        degraded_reasons: Vec::new(),
        memory_max_mb: None,
        memory_swap_max_mb: None,
        pids_max: None,
        readonly_project_root: false,
        project_root: None,
        soft_limit_percent: None,
        memory_monitor_interval_seconds: None,
        user_daemon_ipc: false,
    };
    let args = command_args(
        &from_isolation_plan(&plan, "/usr/bin/tool", &[])
            .expect("valid bind paths")
            .expect("bwrap isolation plan"),
    );
    let replaced = replaced_target.to_string_lossy();
    let dest = readable_symlink.to_string_lossy();

    assert!(
        ro_bind_destination(&args, &dest).is_some(),
        "bind must retain the validated descriptor at the requested destination; args: {args:?}"
    );
    assert!(
        !args
            .windows(3)
            .any(|window| { window[0] == "--ro-bind" && window[1] == replaced }),
        "replaced symlink target must not become the bind source; args: {args:?}"
    );
}
