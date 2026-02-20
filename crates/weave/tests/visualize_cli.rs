use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

fn weave_cmd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_weave"))
}

#[test]
fn visualize_flags_conflict_between_mermaid_and_png() {
    let output = weave_cmd()
        .args([
            "visualize",
            "missing.plan.toml",
            "--mermaid",
            "--png",
            "out.png",
        ])
        .output()
        .expect("run weave visualize with conflicting flags");

    assert!(
        !output.status.success(),
        "command should fail when mutually exclusive flags are combined"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--mermaid"));
    assert!(stderr.contains("--png"));
}

#[test]
fn visualize_missing_input_file_reports_read_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let missing = tmp.path().join("not-found.plan.toml");
    let output = weave_cmd()
        .arg("visualize")
        .arg(&missing)
        .output()
        .expect("run weave visualize for missing file");

    assert!(
        !output.status.success(),
        "missing input file should return non-zero"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("failed to read"));
    assert!(stderr.contains("not-found.plan.toml"));
}

#[test]
fn visualize_malformed_toml_reports_parse_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let plan = tmp.path().join("bad.plan.toml");
    fs::write(&plan, "this is not toml = [").expect("write malformed toml");

    let output = weave_cmd()
        .arg("visualize")
        .arg(&plan)
        .output()
        .expect("run weave visualize for malformed toml");

    assert!(
        !output.status.success(),
        "malformed TOML should return non-zero"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("failed to parse"));
    assert!(stderr.contains("bad.plan.toml"));
}

#[test]
fn visualize_reads_plan_from_stdin_when_dash_is_used() {
    use weave::compiler::{ExecutionPlan, FailAction, PlanStep, plan_to_toml};

    let plan = ExecutionPlan {
        name: "stdin-plan".to_string(),
        description: String::new(),
        variables: Vec::new(),
        steps: vec![PlanStep {
            id: 1,
            title: "Build".to_string(),
            tool: Some("codex".to_string()),
            prompt: "Build project".to_string(),
            tier: None,
            depends_on: Vec::new(),
            on_fail: FailAction::Abort,
            condition: None,
            loop_var: None,
            session: None,
        }],
    };
    let plan_toml = plan_to_toml(&plan).expect("serialize plan toml");

    let mut child = weave_cmd()
        .arg("visualize")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn weave visualize -");

    {
        let stdin = child.stdin.as_mut().expect("stdin pipe should exist");
        stdin
            .write_all(plan_toml.as_bytes())
            .expect("write plan toml to stdin");
    }

    let output = child
        .wait_with_output()
        .expect("wait for weave visualize -");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "visualize from stdin should succeed: {stderr}"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Build"),
        "ascii output should include step title, got: {stdout}"
    );
}

#[cfg(feature = "visualize-png-dot")]
#[test]
fn visualize_png_writes_file_when_dot_is_available() {
    use weave::compiler::{ExecutionPlan, FailAction, PlanStep};

    if which::which("dot").is_err() {
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let png = tmp.path().join("out.png");
    let plan = ExecutionPlan {
        name: "demo".to_string(),
        description: String::new(),
        variables: Vec::new(),
        steps: vec![PlanStep {
            id: 1,
            title: "Build".to_string(),
            tool: None,
            prompt: "Build project".to_string(),
            tier: None,
            depends_on: Vec::new(),
            on_fail: FailAction::Abort,
            condition: None,
            loop_var: None,
            session: None,
        }],
    };

    weave::visualize::render_png(&plan, &png).expect("png rendering should succeed");

    let meta = fs::metadata(&png).expect("png file metadata");
    assert!(meta.len() > 0, "png output should be non-empty");
}
