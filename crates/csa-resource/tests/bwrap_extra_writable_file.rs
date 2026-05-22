use std::path::Path;
use std::process::Command;

use csa_resource::BwrapCommandBuilder;

fn command_args(cmd: &Command) -> Vec<String> {
    format!("{cmd:?}")
        .split('"')
        .enumerate()
        .filter_map(|(i, s)| if i % 2 == 1 { Some(s.to_owned()) } else { None })
        .collect()
}

#[test]
fn bwrap_extra_writable_tmp_file_does_not_create_directory_target() {
    let mut builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
    builder.with_writable_path(Path::new("/tmp/e2e-test-state.json"));
    let cmd = builder.build();
    let args = command_args(&cmd);

    assert!(
        !args
            .windows(2)
            .any(|window| window[0] == "--dir" && window[1] == "/tmp/e2e-test-state.json"),
        "file writable path must not be created as a sandbox directory; args: {args:?}"
    );
    assert!(
        args.windows(3).any(|window| {
            window[0] == "--bind"
                && window[1] == "/tmp/e2e-test-state.json"
                && window[2] == "/tmp/e2e-test-state.json"
        }),
        "file writable path should still be bind-mounted; args: {args:?}"
    );
}

#[test]
fn bwrap_extra_writable_nested_tmp_file_creates_parent_only() {
    let mut builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
    builder.with_writable_path(Path::new("/tmp/csa/state.json"));
    let cmd = builder.build();
    let args = command_args(&cmd);

    assert!(
        args.windows(2)
            .any(|window| window[0] == "--dir" && window[1] == "/tmp/csa"),
        "nested file writable path should create its parent in tmpfs; args: {args:?}"
    );
    assert!(
        !args
            .windows(2)
            .any(|window| window[0] == "--dir" && window[1] == "/tmp/csa/state.json"),
        "nested file writable path must not create the file path as a directory; args: {args:?}"
    );
}
