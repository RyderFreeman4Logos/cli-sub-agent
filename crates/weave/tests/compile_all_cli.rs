use std::fs;
use std::process::Command;

fn weave_cmd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_weave"))
}

#[test]
fn compile_all_subcommand_parses_and_runs_on_empty_dir() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output = weave_cmd()
        .arg("compile-all")
        .arg("--dir")
        .arg(tmp.path())
        .output()
        .expect("run weave compile-all");

    assert!(
        output.status.success(),
        "compile-all on empty dir should succeed (0 patterns is OK)"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no plan.toml"),
        "should report no plan.toml found, got: {stderr}"
    );
}

#[test]
fn compile_all_compiles_fixture_patterns_with_progress() {
    let tmp = tempfile::tempdir().expect("tempdir");

    // Create two valid patterns.
    let p1 = tmp.path().join("alpha");
    let p2 = tmp.path().join("beta");
    fs::create_dir_all(&p1).unwrap();
    fs::create_dir_all(&p2).unwrap();

    let plan_toml = r#"[plan]
name = "test"

[[plan.steps]]
id = 1
title = "Hello"
prompt = "Say hello"
on_fail = "abort"
"#;

    fs::write(p1.join("plan.toml"), plan_toml).unwrap();
    fs::write(p2.join("plan.toml"), plan_toml).unwrap();

    let output = weave_cmd()
        .arg("compile-all")
        .arg("--dir")
        .arg(tmp.path())
        .output()
        .expect("run weave compile-all");

    assert!(
        output.status.success(),
        "compile-all should succeed for valid patterns"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[1/2]"),
        "should show progress [1/2], got: {stderr}"
    );
    assert!(
        stderr.contains("[2/2]"),
        "should show progress [2/2], got: {stderr}"
    );
    assert!(
        stderr.contains("OK"),
        "should report OK for valid patterns, got: {stderr}"
    );
    assert!(
        stderr.contains("2 pattern(s) compiled: 2 OK, 0 FAILED"),
        "should print summary, got: {stderr}"
    );
}

#[test]
fn compile_all_exits_nonzero_when_pattern_fails() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let broken = tmp.path().join("broken");
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join("plan.toml"), "invalid toml [").unwrap();

    let output = weave_cmd()
        .arg("compile-all")
        .arg("--dir")
        .arg(tmp.path())
        .output()
        .expect("run weave compile-all");

    assert!(
        !output.status.success(),
        "compile-all should fail when a pattern is invalid"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("FAILED"),
        "should report FAILED, got: {stderr}"
    );
    assert!(
        stderr.contains("1 FAILED"),
        "summary should show 1 FAILED, got: {stderr}"
    );
}
