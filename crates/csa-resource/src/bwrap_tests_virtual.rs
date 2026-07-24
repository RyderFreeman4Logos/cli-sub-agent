//! Tests for writable paths under bubblewrap's fresh virtual filesystems.

use super::super::BwrapCommandBuilder;
use super::command_args;

#[test]
fn test_bwrap_binds_non_tmp_symlink_writable_path_at_canonical_destination() {
    use std::os::unix::fs::symlink;

    // The quality gate read-only-binds the checkout. Use a writable fixture
    // outside the builder's fresh /tmp and /dev virtual mount roots so this
    // remains a non-virtual-destination regression test.
    let tmp = tempfile::tempdir_in("/var/tmp").expect("tempdir");
    let real = tmp.path().join("real-state");
    std::fs::create_dir(&real).expect("create real dir");
    let canonical_writable = real.join("claude");
    std::fs::create_dir(&canonical_writable).expect("create writable dir");
    let link = tmp.path().join("link-state");
    symlink(&real, &link).expect("create symlink");
    let logical_writable = link.join("claude");

    let mut builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
    builder.with_writable_path(&logical_writable);

    let cmd = builder.build();
    let args = command_args(&cmd);

    let bind_idx = args
        .windows(3)
        .position(|window| window[0] == "--bind")
        .expect("--bind not found");
    let canonical = canonical_writable.to_string_lossy().to_string();
    let canonical_parent = real.to_string_lossy().to_string();
    let logical_parent = link.to_string_lossy().to_string();
    let canonical_dir_idx = args
        .windows(2)
        .position(|window| window[0] == "--dir" && window[1] == canonical_parent)
        .expect("canonical writable parent should be created");
    assert!(
        canonical_dir_idx < bind_idx,
        "canonical parent must be created before the writable bind; args: {args:?}"
    );
    assert!(
        !args
            .windows(2)
            .any(|window| window[0] == "--dir" && window[1] == logical_parent),
        "the logical symlink parent must not be used for --dir; args: {args:?}"
    );
    let src = &args[bind_idx + 1];
    let dest = &args[bind_idx + 2];
    assert_eq!(src, &canonical, "bind source should be canonicalized");
    assert_eq!(
        dest, &canonical,
        "bind destination should be canonicalized so bwrap does not need to create parents through an unresolved state symlink"
    );
}

#[test]
fn test_bwrap_keeps_dev_shm_symlink_writable_path_at_logical_destination() {
    use std::os::unix::fs::symlink;

    let source = tempfile::tempdir().expect("source tempdir");
    let dev_shm = tempfile::tempdir_in("/dev/shm").expect("/dev/shm tempdir");
    let link = dev_shm.path().join("writable-state");
    symlink(source.path(), &link).expect("create /dev/shm symlink");

    let mut builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
    builder.with_writable_path(&link);

    let cmd = builder.build();
    let args = command_args(&cmd);
    let bind_idx = args
        .windows(3)
        .position(|window| window[0] == "--bind")
        .expect("--bind not found");
    let logical_parent = dev_shm.path().to_string_lossy().to_string();
    let canonical_source = source.path().to_string_lossy().to_string();
    let logical_destination = link.to_string_lossy().to_string();
    let logical_parent_dir_idx = args
        .windows(2)
        .position(|window| window[0] == "--dir" && window[1] == logical_parent)
        .expect("logical /dev/shm parent should be created");

    assert!(
        logical_parent_dir_idx < bind_idx,
        "logical /dev/shm parent must be created before the writable bind; args: {args:?}"
    );
    assert_eq!(
        &args[bind_idx + 1],
        &canonical_source,
        "bind source should be canonicalized"
    );
    assert_eq!(
        &args[bind_idx + 2],
        &logical_destination,
        "bind destination must remain under the fresh /dev mount"
    );
}
