#[cfg(unix)]
use std::path::{Path, PathBuf};

#[cfg(unix)]
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

#[cfg(unix)]
fn load_mktd_save_script() -> anyhow::Result<String> {
    let workflow = std::fs::read_to_string(workspace_root().join("patterns/mktd/workflow.toml"))?;
    let plan = weave::compiler::plan_from_toml(&workflow)?;
    let save_step = plan
        .steps
        .iter()
        .find(|step| step.id == 13)
        .ok_or_else(|| anyhow::anyhow!("missing mktd Save TODO step"))?;
    super::extract_bash_code_block(&save_step.prompt)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("Save TODO step missing bash code block"))
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
fn csa_schema_rejecting_stub_script() -> &'static str {
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
    test -s "$spec_file" || exit 66
    if [ "$dry_run" = true ]; then
      if grep -q 'schema_version *= *2' "$spec_file"; then
        echo "Error: unsupported spec schema_version 2; expected 1" >&2
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
fn tricky_todo() -> &'static str {
    r#"# Plan

## Tasks

- [ ] Reject unsupported spec schema versions.
  DONE WHEN: Save TODO fails during dry-run validation before final persist.
"#
}

#[cfg(unix)]
fn unsupported_schema_spec_toml() -> &'static str {
    concat!(
        "schema_version = 2\n",
        "plan_ulid = \"__PLAN_ID__\"\n",
        "summary = \"",
        "\u{62d2}\u{7edd}\u{975e}\u{652f}\u{6301}\u{89c4}\u{683c}\u{3002}",
        "\"\n\n",
        "[[criteria]]\n",
        "kind = \"check\"\n",
        "id = \"check-schema-version\"\n",
        "description = \"Save TODO rejects unsupported spec schema versions.\"\n",
        "status = \"pending\"\n",
    )
}

#[cfg(unix)]
#[test]
fn mktd_save_step_rejects_unsupported_schema_before_persist() -> anyhow::Result<()> {
    let save_script = load_mktd_save_script()?;
    let project_dir = tempfile::tempdir()?;
    let session_dir = tempfile::tempdir()?;
    let bin_dir = tempfile::tempdir()?;
    let csa_stub = bin_dir.path().join("csa");
    std::fs::write(&csa_stub, csa_schema_rejecting_stub_script())?;
    make_executable(&csa_stub)?;
    install_save_script_path_tools(bin_dir.path())?;

    run_git(project_dir.path(), &["init"])?;
    run_git(project_dir.path(), &["checkout", "-b", "fix/2439-schema"])?;

    let output = std::process::Command::new("bash")
        .arg("-c")
        .arg(save_script)
        .current_dir(project_dir.path())
        .env("PATH", bin_dir.path())
        .env("CSA_SESSION_DIR", session_dir.path())
        .env("STEP_12_OUTPUT", tricky_todo())
        .env("STEP_8_OUTPUT", unsupported_schema_spec_toml())
        .env("STEP_2_OUTPUT", "English")
        .env("FEATURE", "unsupported schema")
        .output()?;
    assert!(
        !output.status.success(),
        "unsupported spec schema should fail before persist"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    for required in [
        "spec producer-contract error",
        "generated TODO/spec artifacts failed csa todo persist --dry-run",
        "unsupported spec schema_version 2; expected 1",
        "Spec artifact path:",
        "Raw spec artifact path:",
    ] {
        assert!(
            stderr.contains(required),
            "unsupported schema diagnostic should contain {required}: {stderr}"
        );
    }
    assert!(
        !stderr.contains("csa todo persist failed"),
        "unsupported schema should fail before final csa todo persist: {stderr}"
    );
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains(".todos/"),
        "Save TODO should not print a persisted TODO path after dry-run rejection"
    );

    Ok(())
}
