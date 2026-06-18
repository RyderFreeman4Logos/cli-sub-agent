use std::path::{Path, PathBuf};

use weave::compiler::plan_from_toml;

use crate::plan_cmd::extract_bash_code_block;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

#[cfg(unix)]
#[test]
fn mktd_save_step_persists_issue_quoted_content_without_shell_parse_break() -> anyhow::Result<()> {
    let workflow_path = workspace_root().join("patterns/mktd/workflow.toml");
    let workflow = std::fs::read_to_string(&workflow_path)?;
    let plan = plan_from_toml(&workflow)?;
    let save_step = plan
        .steps
        .iter()
        .find(|step| step.id == 13)
        .expect("missing mktd save step");
    let save_script =
        extract_bash_code_block(&save_step.prompt).expect("mktd save step must have bash block");

    assert!(
        !save_script.contains("```"),
        "Save TODO bash source must not contain markdown fence literals; simple fence extractors can truncate them into an unmatched single quote"
    );
    let simple_extracted = simple_first_fence_body(&save_step.prompt)
        .expect("simple extractor must find the mktd save bash block");
    assert_eq!(
        simple_extracted, save_script,
        "Save TODO must stay robust for simple fence extractors used by older generated-command paths"
    );
    let parse_status = std::process::Command::new("bash")
        .args(["-n", "-c", simple_extracted])
        .status()?;
    assert!(
        parse_status.success(),
        "Save TODO script must parse after simple fence extraction; #2041 failed as an unexpected EOF in a single-quoted sed expression"
    );

    let project_dir = tempfile::tempdir()?;
    let session_dir = tempfile::tempdir()?;
    let bin_dir = tempfile::tempdir()?;
    let csa_stub = bin_dir.path().join("csa");
    std::fs::write(&csa_stub, csa_stub_script())?;
    make_executable(&csa_stub)?;

    run_git(project_dir.path(), &["init"])?;
    run_git(project_dir.path(), &["checkout", "-b", "fix/2041-test"])?;

    let existing_path = std::env::var("PATH").unwrap_or_default();
    let output = std::process::Command::new("bash")
        .arg("-c")
        .arg(save_script)
        .current_dir(project_dir.path())
        .env(
            "PATH",
            format!("{}:{existing_path}", bin_dir.path().display()),
        )
        .env("CSA_SESSION_DIR", session_dir.path())
        .env("STEP_12_OUTPUT", tricky_todo())
        .env("STEP_8_OUTPUT", spec_toml())
        .env("STEP_2_OUTPUT", "English")
        .env("FEATURE", tricky_feature())
        .output()?;
    assert!(
        output.status.success(),
        "Save TODO script failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let todo_artifact =
        std::fs::read_to_string(session_dir.path().join("output/mktd-save/TODO.md"))?;
    assert!(todo_artifact.contains("status = \"failure\""));
    assert!(todo_artifact.contains("failing_step = 'just find-monolith-files'"));
    assert!(todo_artifact.contains("command = `csa session wait`"));
    assert!(todo_artifact.contains("```text"));
    assert!(todo_artifact.contains("```epic-plan.toml"));

    let spec_artifact =
        std::fs::read_to_string(session_dir.path().join("output/mktd-save/spec.toml"))?;
    assert!(spec_artifact.contains("plan_ulid = \"01J2041SHELLQUOTE0000000000\""));

    Ok(())
}

#[cfg(unix)]
fn simple_first_fence_body(prompt: &str) -> Option<&str> {
    let fence_start = prompt.find("```bash")?;
    let code_start = prompt[fence_start..].find('\n')? + fence_start + 1;
    let rest = &prompt[code_start..];
    let fence_end = rest.find("```")?;
    Some(rest[..fence_end].trim())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(unix)]
fn run_git(project_dir: &Path, args: &[&str]) -> anyhow::Result<()> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(project_dir)
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[cfg(unix)]
fn csa_stub_script() -> &'static str {
    r#"#!/bin/sh
set -eu
plan_id='01J2041SHELLQUOTE0000000000'
if [ "${1:-}" != "todo" ]; then
  echo "unexpected csa command: $*" >&2
  exit 64
fi
shift
case "${1:-}" in
  create)
    printf '%s\n' "$plan_id"
    ;;
  persist)
    todo_file=''
    spec_file=''
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --todo-file) shift; todo_file="${1:-}" ;;
        --spec-file) shift; spec_file="${1:-}" ;;
        --epic-plan-file) shift; test -s "${1:-}" || exit 67 ;;
      esac
      shift || true
    done
    test -s "$todo_file" || exit 65
    test -s "$spec_file" || exit 66
    printf '%s/.todos/%s/TODO.md\n' "$PWD" "$plan_id"
    ;;
  show)
    printf '%s/.todos/%s/TODO.md\n' "$PWD" "$plan_id"
    ;;
  *)
    echo "unexpected csa todo command: $*" >&2
    exit 64
    ;;
esac
"#
}

#[cfg(unix)]
fn tricky_todo() -> &'static str {
    r#"# Plan

## Tasks

- [ ] Preserve issue body excerpts safely.
  DONE WHEN: Save TODO persists TODO.md with `status = "failure"`, 'single quotes', and fenced snippets intact.

Issue excerpt:

```text
status = "failure"
summary = "POST-EXEC GATE FAILED (exit=1, step=just find-monolith-files)"
failing_step = 'just find-monolith-files'
command = `csa session wait`
```

```epic-plan.toml
stories = []
note = "don't break shell parsing"
```
"#
}

#[cfg(unix)]
fn spec_toml() -> &'static str {
    concat!(
        "schema_version = 1\n",
        "plan_ulid = \"__PLAN_ID__\"\n",
        "summary = \"",
        "\u{4fdd}\u{5b58}\u{5f15}\u{53f7}\u{548c}\u{4ee3}\u{7801}\u{5757}\u{3002}",
        "\"\n\n",
        "[[criteria]]\n",
        "kind = \"check\"\n",
        "id = \"check-shell-safe\"\n",
        "description = \"Save TODO preserves quoted issue excerpts.\"\n",
        "status = \"pending\"\n",
    )
}

#[cfg(unix)]
fn tricky_feature() -> &'static str {
    "Issue #2041: user's `result.toml` says status = \"failure\"\nsummary = \"POST-EXEC GATE FAILED\""
}
