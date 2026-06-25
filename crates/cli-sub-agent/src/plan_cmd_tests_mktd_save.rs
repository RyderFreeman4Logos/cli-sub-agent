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
    install_save_script_path_tools(bin_dir.path())?;

    run_git(project_dir.path(), &["init"])?;
    run_git(project_dir.path(), &["checkout", "-b", "fix/2041-test"])?;

    let output = std::process::Command::new("bash")
        .arg("-c")
        .arg(save_script)
        .current_dir(project_dir.path())
        .env("PATH", bin_dir.path())
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
#[test]
fn mktd_save_step_normalizes_wrapped_spec_toml_before_persist() -> anyhow::Result<()> {
    for (case_name, step_8_output) in [
        (
            "prose-leading fenced TOML",
            format!(
                "I will now provide the requested spec artifact.\n\n```toml\n{}\n```\n",
                spec_toml()
            ),
        ),
        (
            "CSA-section wrapped fenced TOML",
            format!(
                "<!-- CSA:SECTION:summary -->\n```toml\n{}\n```\n<!-- CSA:SECTION:summary:END -->\n",
                spec_toml()
            ),
        ),
        (
            "CSA-section wrapped raw TOML",
            format!(
                "<!-- CSA:SECTION:summary -->\n{}\n<!-- CSA:SECTION:summary:END -->\n",
                spec_toml()
            ),
        ),
        (
            "CSA details section fenced TOML",
            format!(
                "<!-- CSA:SECTION:details -->\nProducer note outside the artifact.\n\n```toml\n{}\n```\n<!-- CSA:SECTION:details:END -->\n",
                spec_toml()
            ),
        ),
    ] {
        let save_script = load_mktd_save_script()?;
        let project_dir = tempfile::tempdir()?;
        let session_dir = tempfile::tempdir()?;
        let bin_dir = tempfile::tempdir()?;
        let csa_stub = bin_dir.path().join("csa");
        std::fs::write(&csa_stub, csa_stub_script())?;
        make_executable(&csa_stub)?;
        install_save_script_path_tools(bin_dir.path())?;

        run_git(project_dir.path(), &["init"])?;
        run_git(project_dir.path(), &["checkout", "-b", "fix/2375-test"])?;

        let output = std::process::Command::new("bash")
            .arg("-c")
            .arg(&save_script)
            .current_dir(project_dir.path())
            .env("PATH", bin_dir.path())
            .env("CSA_SESSION_DIR", session_dir.path())
            .env("STEP_12_OUTPUT", tricky_todo())
            .env("STEP_8_OUTPUT", step_8_output)
            .env("STEP_2_OUTPUT", "English")
            .env("FEATURE", case_name)
            .output()?;
        assert!(
            output.status.success(),
            "{case_name} should normalize before persist\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let spec_artifact =
            std::fs::read_to_string(session_dir.path().join("output/mktd-save/spec.toml"))?;
        assert!(
            spec_artifact.starts_with("schema_version = 1\n"),
            "{case_name} should write raw TOML spec.toml: {spec_artifact}"
        );
        assert!(
            !spec_artifact.contains("```")
                && !spec_artifact.contains("CSA:SECTION")
                && !spec_artifact.contains("I will now provide"),
            "{case_name} should strip wrapper/prose from spec.toml: {spec_artifact}"
        );

        let raw_spec =
            std::fs::read_to_string(session_dir.path().join("output/mktd-save/spec.raw.txt"))?;
        assert!(
            raw_spec.contains("```toml") || raw_spec.contains("CSA:SECTION"),
            "{case_name} should preserve the raw producer output for diagnostics"
        );
    }

    Ok(())
}

#[cfg(unix)]
#[test]
fn mktd_save_step_accepts_parser_valid_noncanonical_spec_toml() -> anyhow::Result<()> {
    let save_script = load_mktd_save_script()?;
    let project_dir = tempfile::tempdir()?;
    let session_dir = tempfile::tempdir()?;
    let bin_dir = tempfile::tempdir()?;
    let csa_stub = bin_dir.path().join("csa");
    std::fs::write(&csa_stub, csa_stub_script())?;
    make_executable(&csa_stub)?;
    install_save_script_path_tools(bin_dir.path())?;

    run_git(project_dir.path(), &["init"])?;
    run_git(
        project_dir.path(),
        &["checkout", "-b", "fix/2439-noncanonical"],
    )?;

    let output = std::process::Command::new("bash")
        .arg("-c")
        .arg(save_script)
        .current_dir(project_dir.path())
        .env("PATH", bin_dir.path())
        .env("CSA_SESSION_DIR", session_dir.path())
        .env("STEP_12_OUTPUT", tricky_todo())
        .env("STEP_8_OUTPUT", noncanonical_spec_toml())
        .env("STEP_2_OUTPUT", "English")
        .env("FEATURE", "parser-valid noncanonical TOML")
        .output()?;
    assert!(
        output.status.success(),
        "parser-valid noncanonical TOML should pass Save TODO\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let spec_artifact =
        std::fs::read_to_string(session_dir.path().join("output/mktd-save/spec.toml"))?;
    assert!(
        spec_artifact.starts_with("schema_version=1\n"),
        "Save TODO should preserve parser-valid TOML instead of requiring canonical spacing: {spec_artifact}"
    );
    assert!(
        spec_artifact.contains("kind='check'"),
        "Save TODO should accept TOML single-quoted strings after dry-run validation: {spec_artifact}"
    );

    Ok(())
}

#[cfg(unix)]
#[test]
fn mktd_save_step_rejects_unrecoverable_prose_spec_before_persist() -> anyhow::Result<()> {
    let save_script = load_mktd_save_script()?;
    let project_dir = tempfile::tempdir()?;
    let session_dir = tempfile::tempdir()?;
    let bin_dir = tempfile::tempdir()?;
    let csa_stub = bin_dir.path().join("csa");
    std::fs::write(&csa_stub, csa_stub_script())?;
    make_executable(&csa_stub)?;
    install_save_script_path_tools(bin_dir.path())?;

    run_git(project_dir.path(), &["init"])?;
    run_git(project_dir.path(), &["checkout", "-b", "fix/2375-bad-spec"])?;

    let output = std::process::Command::new("bash")
        .arg("-c")
        .arg(save_script)
        .current_dir(project_dir.path())
        .env("PATH", bin_dir.path())
        .env("CSA_SESSION_DIR", session_dir.path())
        .env("STEP_12_OUTPUT", tricky_todo())
        .env(
            "STEP_8_OUTPUT",
            "I will describe the plan here instead of emitting TOML.",
        )
        .env("STEP_2_OUTPUT", "English")
        .env("FEATURE", "unrecoverable prose spec")
        .output()?;
    assert!(
        !output.status.success(),
        "unrecoverable prose spec should fail before persist"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("spec producer-contract error"),
        "Save TODO should report a producer-contract diagnostic: {stderr}"
    );
    assert!(
        stderr.contains("expected TOML spec artifact"),
        "Save TODO should state the expected artifact contract: {stderr}"
    );
    assert!(
        stderr.contains("first content: non-TOML"),
        "Save TODO should classify unrecoverable prose: {stderr}"
    );
    assert!(
        stderr.contains("parser/root-cause:"),
        "Save TODO should expose the extraction root cause: {stderr}"
    );
    assert!(
        stderr.contains("Raw spec artifact path:"),
        "Save TODO should point at the bounded raw artifact: {stderr}"
    );
    assert!(
        !stderr.contains("csa todo persist failed"),
        "Save TODO must fail before csa todo persist: {stderr}"
    );

    Ok(())
}

#[cfg(unix)]
#[test]
fn mktd_save_step_rejects_truncated_spec_with_parser_root_cause() -> anyhow::Result<()> {
    let save_script = load_mktd_save_script()?;
    let project_dir = tempfile::tempdir()?;
    let session_dir = tempfile::tempdir()?;
    let bin_dir = tempfile::tempdir()?;
    let csa_stub = bin_dir.path().join("csa");
    std::fs::write(&csa_stub, csa_stub_script())?;
    make_executable(&csa_stub)?;
    install_save_script_path_tools(bin_dir.path())?;

    run_git(project_dir.path(), &["init"])?;
    run_git(
        project_dir.path(),
        &["checkout", "-b", "fix/2439-truncated"],
    )?;

    let output = std::process::Command::new("bash")
        .arg("-c")
        .arg(save_script)
        .current_dir(project_dir.path())
        .env("PATH", bin_dir.path())
        .env("CSA_SESSION_DIR", session_dir.path())
        .env("STEP_12_OUTPUT", tricky_todo())
        .env(
            "STEP_8_OUTPUT",
            "schema_version = 1\nplan_ulid = \"__PLAN_ID__\"\nsummary = \"truncated but sentinel-complete\"\n\n[[criteria]]\nkind = \"scenario\"\nid = \"S1\"\ndescription = \"unterminated",
        )
        .env("STEP_2_OUTPUT", "English")
        .env("FEATURE", "truncated spec")
        .output()?;
    assert!(
        !output.status.success(),
        "truncated spec should fail before persist"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    for required in [
        "spec producer-contract error",
        "parser/root-cause:",
        "TOML parse error",
        "Spec artifact path:",
        "Raw spec artifact path:",
    ] {
        assert!(
            stderr.contains(required),
            "truncated spec diagnostic should contain {required}: {stderr}"
        );
    }
    assert!(
        !stderr.contains("csa todo persist failed"),
        "truncated spec should fail before csa todo persist: {stderr}"
    );

    Ok(())
}

#[cfg(unix)]
#[test]
fn mktd_save_step_rejects_command_stderr_contaminated_spec() -> anyhow::Result<()> {
    let save_script = load_mktd_save_script()?;
    let project_dir = tempfile::tempdir()?;
    let session_dir = tempfile::tempdir()?;
    let bin_dir = tempfile::tempdir()?;
    let csa_stub = bin_dir.path().join("csa");
    std::fs::write(&csa_stub, csa_stub_script())?;
    make_executable(&csa_stub)?;
    install_save_script_path_tools(bin_dir.path())?;

    run_git(project_dir.path(), &["init"])?;
    run_git(
        project_dir.path(),
        &["checkout", "-b", "fix/2439-contaminated"],
    )?;

    let output = std::process::Command::new("bash")
        .arg("-c")
        .arg(save_script)
        .current_dir(project_dir.path())
        .env("PATH", bin_dir.path())
        .env("CSA_SESSION_DIR", session_dir.path())
        .env("STEP_12_OUTPUT", tricky_todo())
        .env(
            "STEP_8_OUTPUT",
            format!(
                "error: failed to create cargo target dir: Read-only file system (os error 30)\n\n```toml\n{}\n```\n",
                spec_toml()
            ),
        )
        .env("STEP_2_OUTPUT", "English")
        .env("FEATURE", "stderr contaminated spec")
        .output()?;
    assert!(
        !output.status.success(),
        "stderr-contaminated spec should fail before persist"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    for required in [
        "first content: diag",
        "underlying command failure",
        "Read-only file system",
        "Command stderr summary:",
        "Spec artifact path:",
        "Raw spec artifact path:",
    ] {
        assert!(
            stderr.contains(required),
            "contaminated spec diagnostic should contain {required}: {stderr}"
        );
    }
    assert!(
        !stderr.contains("csa todo persist failed"),
        "contaminated spec should fail before csa todo persist: {stderr}"
    );

    Ok(())
}

#[cfg(unix)]
#[test]
fn mktd_save_step_reports_persist_stderr_context_on_failure() -> anyhow::Result<()> {
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

    let project_dir = tempfile::tempdir()?;
    let session_dir = tempfile::tempdir()?;
    let bin_dir = tempfile::tempdir()?;
    let csa_stub = bin_dir.path().join("csa");
    std::fs::write(&csa_stub, csa_failing_persist_stub_script())?;
    make_executable(&csa_stub)?;
    install_save_script_path_tools(bin_dir.path())?;

    run_git(project_dir.path(), &["init"])?;
    run_git(
        project_dir.path(),
        &["checkout", "-b", "fix/persist-detail-test"],
    )?;

    let output = std::process::Command::new("bash")
        .arg("-c")
        .arg(save_script)
        .current_dir(project_dir.path())
        .env("PATH", bin_dir.path())
        .env("CSA_SESSION_DIR", session_dir.path())
        .env("STEP_12_OUTPUT", tricky_todo())
        .env("STEP_8_OUTPUT", spec_toml())
        .env("STEP_2_OUTPUT", "English")
        .env("FEATURE", "persist failure detail")
        .output()?;
    assert!(
        !output.status.success(),
        "Save TODO should fail when csa todo persist rejects the artifacts"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("csa todo persist failed (exit 1)"),
        "Save TODO stderr should preserve the persist wrapper: {stderr}"
    );
    assert!(
        stderr.contains("Spec artifact path:"),
        "Save TODO stderr should include bounded artifact context: {stderr}"
    );
    assert!(
        stderr.contains("Persist stderr artifact:"),
        "Save TODO stderr should point at the persisted stderr excerpt: {stderr}"
    );
    assert!(
        stderr.contains("failed to parse spec file"),
        "Save TODO stderr should replay concrete persist stderr: {stderr}"
    );
    assert!(
        stderr.contains("TOML parse error at line 6, column 1"),
        "Save TODO stderr should replay TOML line/column detail: {stderr}"
    );

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
fn load_mktd_save_script() -> anyhow::Result<String> {
    let workflow_path = workspace_root().join("patterns/mktd/workflow.toml");
    let workflow = std::fs::read_to_string(&workflow_path)?;
    let plan = plan_from_toml(&workflow)?;
    let save_step = plan
        .steps
        .iter()
        .find(|step| step.id == 13)
        .expect("missing mktd save step");
    extract_bash_code_block(&save_step.prompt)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("mktd save step must have bash block"))
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
fn install_save_script_path_tools(bin_dir: &Path) -> anyhow::Result<()> {
    for tool in [
        "awk", "bash", "git", "grep", "head", "mkdir", "sed", "tail", "tr", "wc", "perl",
    ] {
        let source = resolve_path_tool(tool)?;
        std::os::unix::fs::symlink(&source, bin_dir.join(tool))?;
    }
    Ok(())
}

#[cfg(unix)]
fn resolve_path_tool(tool: &str) -> anyhow::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let candidate = dir.join(tool);
        let Ok(metadata) = std::fs::metadata(&candidate) else {
            continue;
        };
        if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
            return Ok(candidate);
        }
    }
    anyhow::bail!("required test tool not found on PATH: {tool}")
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
    dry_run=false
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --dry-run) dry_run=true ;;
        --todo-file) shift; todo_file="${1:-}" ;;
        --spec-file) shift; spec_file="${1:-}" ;;
        --epic-plan-file) shift; test -s "${1:-}" || exit 67 ;;
      esac
      shift || true
    done
    test -s "$todo_file" || exit 65
    test -s "$spec_file" || exit 66
    if [ "$dry_run" = true ]; then
      if grep -q 'description = "unterminated' "$spec_file"; then
        echo "Error: failed to parse spec file '${spec_file}': TOML parse error at line 8, column 1" >&2
        echo "invalid basic string" >&2
        exit 1
      fi
      exit 0
    fi
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
fn csa_failing_persist_stub_script() -> &'static str {
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
    spec_file=''
    dry_run=false
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --dry-run) dry_run=true ;;
        --spec-file) shift; spec_file="${1:-}" ;;
      esac
      shift || true
    done
    if [ "$dry_run" = true ]; then
      exit 0
    fi
    echo "Error: failed to parse spec file '${spec_file}': TOML parse error at line 6, column 1" >&2
    echo "invalid table header" >&2
    exit 1
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
fn noncanonical_spec_toml() -> &'static str {
    concat!(
        "schema_version=1\n",
        "plan_ulid='__PLAN_ID__'\n",
        "summary='",
        "\u{4fdd}\u{5b58}\u{975e}\u{89c4}\u{8303}\u{683c}\u{5f0f}\u{3002}",
        "'\n\n",
        "[[criteria]]\n",
        "kind='check'\n",
        "id='check-noncanonical'\n",
        "description='Save TODO accepts parser-valid TOML without canonical spacing.'\n",
        "status='pending'\n",
    )
}

#[cfg(unix)]
fn tricky_feature() -> &'static str {
    "Issue #2041: user's `result.toml` says status = \"failure\"\nsummary = \"POST-EXEC GATE FAILED\""
}
