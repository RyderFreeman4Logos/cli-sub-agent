use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SESSION_ID: &str = "019c871c-b1f9-7f60-9c4f-87ed09f13592";
const LARGE_SESSION_ID: &str = "019c871d-0253-7130-8e11-0c1a4a0beef1";

fn csa_cmd(tmp: &Path, project: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_csa"));
    scrub_inherited_csa_env(&mut cmd);
    cmd.env("HOME", tmp)
        .env("XDG_STATE_HOME", tmp.join(".local/state"))
        .env("XDG_CONFIG_HOME", tmp.join(".config"))
        .env("CODEX_HOME", tmp.join(".codex"))
        .env("TOKIO_WORKER_THREADS", "1")
        .current_dir(project);
    cmd
}

fn scrub_inherited_csa_env(cmd: &mut Command) {
    for (key, _) in std::env::vars_os() {
        let key = key.to_string_lossy();
        if key.starts_with("CSA_")
            || key == "CODEX_HOME"
            || key == "CLAUDE_CONFIG_DIR"
            || key == "GEMINI_CLI_HOME"
            || key == "OPENCODE_DATA_DIR"
        {
            cmd.env_remove(key.as_ref());
        }
    }
}

fn write_codex_rollout(tmp: &Path, session_id: &str, lines: &[String]) -> PathBuf {
    let path = tmp
        .join(".codex")
        .join("sessions/2026/06/30")
        .join(format!("rollout-2026-06-30T12-00-00-{session_id}.jsonl"));
    std::fs::create_dir_all(path.parent().expect("rollout parent")).expect("create rollout dir");
    std::fs::write(&path, format!("{}\n", lines.join("\n"))).expect("write rollout");
    path
}

fn write_conflicting_claude_history(tmp: &Path, project: &Path) {
    let path = if cfg!(target_os = "macos") {
        tmp.join("Library/Application Support/cli-sub-agent/main-agent-history.jsonl")
    } else {
        tmp.join(".local/state/cli-sub-agent/main-agent-history.jsonl")
    };
    std::fs::create_dir_all(path.parent().expect("history parent")).expect("create history dir");
    std::fs::write(
        path,
        format!(
            r#"{{"ts":"2026-06-30T12:00:00Z","sid":"{SESSION_ID}","project":"{}","provider":"claude"}}"#,
            project.display()
        ),
    )
    .expect("write history");
}

fn visible_fixture_lines(project: &Path) -> Vec<String> {
    vec![
        format!(
            r#"{{"type":"session_meta","timestamp":"2026-06-30T12:00:00Z","payload":{{"cwd":"{}"}}}}"#,
            project.display()
        ),
        r#"{"type":"response_item","timestamp":"2026-06-30T12:00:01Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"visible user alpha"}]}}"#.to_string(),
        r#"{"type":"response_item","timestamp":"2026-06-30T12:00:02Z","payload":{"type":"function_call","name":"shell","arguments":"{\"cmd\":\"echo secret-tool-payload\"}"}}"#.to_string(),
        r#"{"type":"response_item","timestamp":"2026-06-30T12:00:03Z","payload":{"type":"function_call_output","output":"secret-shell-token raw output"}}"#.to_string(),
        r#"{"type":"event_msg","timestamp":"2026-06-30T12:00:04Z","payload":{"type":"agent_message","message":"visible assistant beta"}}"#.to_string(),
        r#"{"type":"response_item","timestamp":"2026-06-30T12:00:05Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"final assistant gamma"}]}}"#.to_string(),
    ]
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn codex_provider_session_reads_visible_messages_and_ignores_history_provider() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).expect("create project");
    write_codex_rollout(tmp.path(), SESSION_ID, &visible_fixture_lines(&project));
    write_conflicting_claude_history(tmp.path(), &project);

    let output = csa_cmd(tmp.path(), &project)
        .args([
            "xurl",
            "recall",
            "--provider",
            "codex",
            "--session",
            SESSION_ID,
        ])
        .output()
        .expect("run csa xurl recall codex session");

    assert!(output.status.success(), "stderr={}", stderr(&output));
    let stdout = stdout(&output);
    let stderr = stderr(&output);
    assert!(!stderr.contains("only 'hermes'"), "stderr={stderr}");
    assert!(stdout.contains("visible user alpha"), "stdout={stdout}");
    assert!(stdout.contains("visible assistant beta"), "stdout={stdout}");
    assert!(!stdout.contains("secret-tool-payload"), "stdout={stdout}");
    assert!(!stdout.contains("secret-shell-token"), "stdout={stdout}");
    assert!(!stdout.contains("function_call"), "stdout={stdout}");
}

#[test]
fn codex_provider_page_zero_large_session_is_bounded() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).expect("create project");
    let large_text = format!("large-visible-start {}", "x".repeat(120 * 1024));
    let lines = vec![
        format!(
            r#"{{"type":"session_meta","timestamp":"2026-06-30T12:00:00Z","payload":{{"cwd":"{}"}}}}"#,
            project.display()
        ),
        format!(
            r#"{{"type":"response_item","timestamp":"2026-06-30T12:00:01Z","payload":{{"type":"message","role":"assistant","content":[{{"type":"output_text","text":"{large_text}"}}]}}}}"#
        ),
    ];
    write_codex_rollout(tmp.path(), LARGE_SESSION_ID, &lines);

    let output = csa_cmd(tmp.path(), &project)
        .args([
            "xurl",
            "recall",
            "--provider",
            "codex",
            "--session",
            LARGE_SESSION_ID,
            "--page",
            "0",
        ])
        .output()
        .expect("run csa xurl recall codex page");

    assert!(output.status.success(), "stderr={}", stderr(&output));
    assert!(
        output.stdout.len() <= 48 * 1024,
        "page exceeded hard cap: {} bytes",
        output.stdout.len()
    );
}

#[test]
fn codex_provider_keyword_works_in_session() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).expect("create project");
    write_codex_rollout(tmp.path(), SESSION_ID, &visible_fixture_lines(&project));

    let output = csa_cmd(tmp.path(), &project)
        .args([
            "xurl",
            "recall",
            "--provider",
            "codex",
            "--keyword",
            "beta",
            "--session",
            SESSION_ID,
        ])
        .output()
        .expect("run csa xurl recall codex in-session keyword");

    assert!(output.status.success(), "stderr={}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains("Matches in session"), "stdout={stdout}");
    assert!(stdout.contains("visible assistant beta"), "stdout={stdout}");
    assert!(!stdout.contains("secret-shell-token"), "stdout={stdout}");
}

#[test]
fn codex_provider_keyword_cross_session_uses_visible_preview() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).expect("create project");
    write_codex_rollout(tmp.path(), SESSION_ID, &visible_fixture_lines(&project));

    let output = csa_cmd(tmp.path(), &project)
        .args([
            "xurl",
            "recall",
            "--provider",
            "codex",
            "--keyword",
            "beta",
            "--limit",
            "5",
        ])
        .output()
        .expect("run csa xurl recall codex cross-session keyword");

    assert!(output.status.success(), "stderr={}", stderr(&output));
    let output_stdout = stdout(&output);
    assert!(output_stdout.contains(SESSION_ID), "stdout={output_stdout}");
    assert!(output_stdout.contains("[beta]"), "stdout={output_stdout}");
    assert!(
        !output_stdout.contains("secret-shell-token"),
        "stdout={output_stdout}"
    );

    let hidden_output = csa_cmd(tmp.path(), &project)
        .args([
            "xurl",
            "recall",
            "--provider",
            "codex",
            "--keyword",
            "secret-shell-token",
            "--limit",
            "5",
        ])
        .output()
        .expect("run csa xurl recall hidden keyword");
    assert!(
        hidden_output.status.success(),
        "stderr={}",
        stderr(&hidden_output)
    );
    let hidden_stdout = stdout(&hidden_output);
    assert!(
        hidden_stdout.contains("No matches for keyword 'secret-shell-token'"),
        "stdout={hidden_stdout}"
    );
}

#[test]
fn codex_provider_list_uses_codex_thread_query() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).expect("create project");
    write_codex_rollout(tmp.path(), SESSION_ID, &visible_fixture_lines(&project));

    let output = csa_cmd(tmp.path(), &project)
        .args(["xurl", "recall", "--provider", "codex", "--list"])
        .output()
        .expect("run csa xurl recall codex list");

    assert!(output.status.success(), "stderr={}", stderr(&output));
    let stdout = stdout(&output);
    let stderr = stderr(&output);
    assert!(!stderr.contains("only 'hermes'"), "stderr={stderr}");
    assert!(stdout.contains("codex"), "stdout={stdout}");
    assert!(stdout.contains(SESSION_ID), "stdout={stdout}");
}
