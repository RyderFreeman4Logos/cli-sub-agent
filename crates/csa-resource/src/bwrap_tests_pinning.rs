//! Read-only bind identity and fail-closed regressions for #3174.

use super::*;
use std::os::unix::fs::symlink;

#[test]
fn bad_readonly_bind_returns_error_before_child_spawn() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let missing = root.as_path().join("missing");
    let marker = root.as_path().join("child-started");
    let result: std::io::Result<()> = (|| {
        let mut builder =
            BwrapCommandBuilder::new("/usr/bin/touch", &[marker.display().to_string()]);
        builder.with_ro_bind(&missing, &missing);
        let mut command = builder.build_with_home(None)?;
        command.spawn()?.wait()?;
        Ok(())
    })();
    assert!(!marker.exists());
    assert!(result.is_err(), "bad bind must return a recoverable error");
}

fn bind_fd(command: &Command, dest: &Path) -> String {
    let args = command_args(command);
    args.windows(3)
        .find(|args| args[0] == "--ro-bind-fd" && args[2] == dest.to_string_lossy())
        .unwrap_or_else(|| panic!("missing pinned mount for {}: {args:?}", dest.display()))[1]
        .clone()
}

#[test]
fn pinned_readonly_regular_and_extra_sources_survive_replacement_and_builder_drop() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    for extra in [false, true] {
        let source = root
            .as_path()
            .join(if extra { "extra" } else { "readable" });
        std::fs::write(&source, "accepted").unwrap();
        let mut builder = BwrapCommandBuilder::new("/bin/true", &[]);
        if extra {
            builder.with_ro_bind(&source, &source);
        } else {
            builder.with_readable_path(&source);
        }
        std::fs::rename(&source, source.with_extension("old")).unwrap();
        std::fs::write(&source, "replacement").unwrap();
        let command = builder.build_with_home(None).expect("valid bind paths");
        let fd = bind_fd(&command, &source);
        drop(builder);
        assert_eq!(
            std::fs::read_to_string(format!("/proc/self/fd/{fd}")).unwrap(),
            "accepted"
        );
    }
}

#[test]
fn pinned_readonly_rejects_symlink_and_missing_sources() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let real = root.as_path().join("real");
    std::fs::write(&real, "data").unwrap();
    let link = root.as_path().join("link");
    symlink(&real, &link).unwrap();
    for source in [link, root.as_path().join("missing")] {
        for extra in [false, true] {
            let mut builder = BwrapCommandBuilder::new("/bin/true", &[]);
            if extra {
                builder.with_ro_bind(&source, &source);
            } else {
                builder.with_readable_path(&source);
            }
            assert!(
                builder.build_with_home(None).is_err(),
                "{}",
                source.display()
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn pinned_readonly_preserves_explicit_nonregular_path_binds() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let socket = root.as_path().join("control.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let mut builder = BwrapCommandBuilder::new("/bin/true", &[]);
    builder.with_ro_bind(&socket, &socket);
    let command = builder.build_with_home(None).expect("valid bind paths");
    let args = command_args(&command);
    let source = socket.to_string_lossy();
    assert!(
        args.windows(3)
            .any(|window| window[0] == "--ro-bind" && window[1] == source && window[2] == source),
        "intentional nonregular bind must remain pathname-backed; args: {args:?}"
    );
    assert!(
        !args
            .windows(3)
            .any(|window| window[0] == "--ro-bind-fd" && window[2] == source),
        "nonregular bind must not be read-opened for FD mounting; args: {args:?}"
    );
}

#[test]
fn pinned_readonly_implicit_remap_retains_directory_descriptor() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let source = root.as_path().join(".config/gh-aider");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("hosts.yml"), "accepted").unwrap();
    let mut builder = BwrapCommandBuilder::new("/bin/true", &[]);
    builder.with_env("HOME", "/sandbox-home");
    let command = builder
        .build_with_home(Some(root.as_path()))
        .expect("valid bind paths");
    let fd = bind_fd(&command, Path::new("/sandbox-home/.config/gh-aider"));
    std::fs::rename(&source, source.with_extension("old")).unwrap();
    drop(builder);
    assert_eq!(
        std::fs::read_to_string(format!("/proc/self/fd/{fd}/hosts.yml")).unwrap(),
        "accepted"
    );
}
