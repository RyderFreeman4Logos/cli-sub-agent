use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;
use weave::compiler::plan_from_toml;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::plan_cmd::extract_bash_code_block;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn dev2merge_workflow_step_bash(title: &str) -> String {
    let workflow =
        std::fs::read_to_string(workspace_root().join("patterns/dev2merge/workflow.toml")).unwrap();
    let plan = plan_from_toml(&workflow).unwrap();
    let step = plan
        .steps
        .iter()
        .find(|step| step.title == title)
        .unwrap_or_else(|| panic!("missing dev2merge step: {title}"));

    extract_bash_code_block(&step.prompt)
        .unwrap_or_else(|| panic!("missing bash block for dev2merge step: {title}"))
        .trim()
        .to_string()
}

fn dev2merge_bundled_helper(name: &str) -> String {
    std::fs::read_to_string(
        workspace_root()
            .join("patterns/dev2merge/scripts/csa")
            .join(name),
    )
    .unwrap_or_else(|error| panic!("read bundled dev2merge helper {name}: {error}"))
}

fn markdown_step_section<'a>(content: &'a str, heading: &str) -> &'a str {
    let start = content
        .find(heading)
        .unwrap_or_else(|| panic!("missing markdown step heading: {heading}"));
    let rest = &content[start..];
    let end = rest[1..]
        .find("\n## Step ")
        .map(|offset| offset + 1)
        .unwrap_or(rest.len());
    &rest[..end]
}

fn dev2merge_pattern_step_bash(pattern: &str, heading: &str) -> String {
    let section = markdown_step_section(pattern, heading);
    extract_bash_code_block(section)
        .unwrap_or_else(|| panic!("missing bash block for dev2merge pattern heading: {heading}"))
        .trim()
        .to_string()
}

#[test]
fn dev2merge_2305_changed_bash_blocks_stay_synced() {
    let pattern =
        std::fs::read_to_string(workspace_root().join("patterns/dev2merge/PATTERN.md")).unwrap();

    for (title, heading) in [
        (
            "Cheap Repo Preconditions",
            "## Step 3: Cheap Repo Preconditions",
        ),
        ("FAST_PATH Commit", "## Step 4: FAST_PATH Commit"),
        (
            "FAST_PATH Version Bump",
            "## Step 5: FAST_PATH Version Bump",
        ),
        (
            "FAST_PATH Pre-PR Review",
            "## Step 6: FAST_PATH Pre-PR Review",
        ),
        ("Plan with mktd", "## Step 7: Plan with mktd"),
        ("Resume Commit", "## Step 9: Resume Commit"),
        ("Ensure Version Bumped", "## Step 10: Ensure Version Bumped"),
        (
            "Decomposition Review Depth Warning",
            "## Step 11.5: Decomposition Review Depth Warning",
        ),
        (
            "Pre-PR Cumulative Review Gate",
            "## Step 12: Pre-PR Cumulative Review Gate",
        ),
        (
            "Pre-PR Review Verdict Check",
            "## Step 14: Pre-PR Review Verdict Check",
        ),
    ] {
        assert_eq!(
            dev2merge_pattern_step_bash(&pattern, heading),
            dev2merge_workflow_step_bash(title),
            "dev2merge PATTERN.md and workflow.toml {heading} bash blocks must stay synced"
        );
    }
}

#[test]
fn dev2merge_decomposition_warning_placeholders_stay_synced() {
    let pattern =
        std::fs::read_to_string(workspace_root().join("patterns/dev2merge/PATTERN.md")).unwrap();
    let pattern_bash =
        dev2merge_pattern_step_bash(&pattern, "## Step 11.5: Decomposition Review Depth Warning");
    let workflow_bash = dev2merge_workflow_step_bash("Decomposition Review Depth Warning");

    assert_eq!(
        placeholders_in(&pattern_bash),
        placeholders_in(&workflow_bash),
        "dev2merge Step 11.5 must not introduce orphan ${{VAR}} placeholders"
    );
}

fn placeholders_in(content: &str) -> std::collections::BTreeSet<String> {
    let mut placeholders = std::collections::BTreeSet::new();
    let mut rest = content;
    while let Some(start) = rest.find("${") {
        let after_start = &rest[start + 2..];
        let Some(end) = after_start.find('}') else {
            break;
        };
        placeholders.insert(after_start[..end].to_string());
        rest = &after_start[end + 1..];
    }
    placeholders
}

#[test]
fn dev2merge_cheap_preconditions_run_before_expensive_gates() {
    let workflow =
        std::fs::read_to_string(workspace_root().join("patterns/dev2merge/workflow.toml")).unwrap();
    let plan = plan_from_toml(&workflow).unwrap();
    let step_id = |title: &str| {
        plan.steps
            .iter()
            .find(|step| step.title == title)
            .unwrap_or_else(|| panic!("missing dev2merge step: {title}"))
            .id
    };

    assert!(
        step_id("FAST_PATH Version Bump") < step_id("FAST_PATH Pre-PR Review"),
        "FAST_PATH version bump must run before build/review work"
    );
    assert!(
        step_id("Ensure Version Bumped") < step_id("Self-Review Gate"),
        "full/resume version bump must run before expensive self-review/review gates"
    );

    let cheap = dev2merge_workflow_step_bash("Cheap Repo Preconditions");
    assert!(
        cheap.contains("STAGED_FILES=")
            && cheap.contains("staged-scope precondition failed")
            && cheap.contains("check-version-bumped"),
        "cheap precondition step must cover staged scope and version detection: {cheap}"
    );
    for forbidden in ["just clippy", "just test", "csa review", "cargo build"] {
        assert!(
            !cheap.contains(forbidden),
            "cheap precondition step must not run expensive gate {forbidden}: {cheap}"
        );
    }

    let fast_commit = dev2merge_workflow_step_bash("FAST_PATH Commit");
    assert!(
        fast_commit.contains("git diff --cached --check"),
        "FAST_PATH commit must run staged-scope checks before downstream gates"
    );
    assert!(
        !fast_commit.contains("just test"),
        "FAST_PATH commit must stay cheap; tests move to Step 6 after version bump"
    );

    let fast_review = dev2merge_workflow_step_bash("FAST_PATH Pre-PR Review");
    let version_gate = step_id("FAST_PATH Version Bump");
    let review_gate = step_id("FAST_PATH Pre-PR Review");
    assert!(version_gate < review_gate);
    let clippy_index = fast_review
        .find("just clippy")
        .expect("FAST_PATH review step should run L1 gate before review");
    let review_index = fast_review
        .find("cumulative-review-batch.sh")
        .expect("FAST_PATH review step should run cumulative review");
    assert!(
        clippy_index < review_index,
        "FAST_PATH L1/L2 gates must execute before cumulative review"
    );
}

#[test]
fn dev2merge_cumulative_review_gates_use_bundled_helpers() {
    let helper_dir = workspace_root().join("patterns/dev2merge/scripts/csa");
    assert!(
        helper_dir.join("cumulative-review-batch.sh").is_file(),
        "dev2merge must bundle cumulative-review-batch.sh with the pattern"
    );
    assert!(
        helper_dir.join("session-wait-until-done.sh").is_file(),
        "dev2merge cumulative review helper must bundle its wait dependency"
    );

    let helper = std::fs::read_to_string(helper_dir.join("cumulative-review-batch.sh")).unwrap();
    assert!(
        helper.contains(r#"SESSION_WAIT_SCRIPT="${CSA_SESSION_WAIT_SCRIPT:-${SCRIPT_DIR}/session-wait-until-done.sh}""#),
        "cumulative-review-batch.sh must resolve session-wait relative to itself"
    );
    assert!(
        helper.contains(r#"csa review --check-verdict --range "${default_branch}...HEAD""#),
        "cumulative-review-batch.sh must own the exact-head verdict check after running review"
    );
    assert!(
        !helper.contains("bash scripts/csa/session-wait-until-done.sh"),
        "cumulative-review-batch.sh must not resolve wait helper from the target repo"
    );

    for title in ["FAST_PATH Pre-PR Review", "Pre-PR Cumulative Review Gate"] {
        let script = dev2merge_workflow_step_bash(title);
        assert!(
            script.contains(
                r#"bash "${CSA_WORKFLOW_DIR:-patterns/dev2merge}/scripts/csa/cumulative-review-batch.sh" --default-branch "${DEFAULT_BRANCH}" --"#
            ),
            "{title} must invoke the bundled cumulative review helper through CSA_WORKFLOW_DIR with a source-tree fallback"
        );
        assert!(
            !script.contains("bash scripts/csa/"),
            "{title} must not depend on target-repo-local scripts/csa helpers"
        );
        assert!(
            script.contains(r#"csa review --sa-mode true --range "${DEFAULT_BRANCH}...HEAD""#),
            "{title} must preserve SA-mode exact-range review"
        );
        assert!(
            !script.contains("csa review --check-verdict"),
            "{title} must not run unconditional exact-head check-verdict after the batching helper"
        );

        let review_index = script
            .find("cumulative-review-batch.sh")
            .unwrap_or_else(|| panic!("{title} must run cumulative review"));
        let completed_marker_index = script
            .find("CSA_VAR:REVIEW_COMPLETED=true")
            .unwrap_or_else(|| panic!("{title} must emit REVIEW_COMPLETED"));
        assert!(
            review_index < completed_marker_index,
            "{title} must let the helper finish review/batch gating before emitting REVIEW_COMPLETED"
        );
    }

    let verdict_check = dev2merge_workflow_step_bash("Pre-PR Review Verdict Check");
    assert!(
        verdict_check.contains(r#""${REVIEW_COMPLETED:-}" = "true""#)
            && verdict_check
                .contains(r#"csa review --check-verdict --range "${DEFAULT_BRANCH}...HEAD""#),
        "Step 14 must accept helper completion and keep exact-head check-verdict as a resume fallback"
    );
}

#[test]
fn dev2merge_plan_step_resolves_mktd_by_pattern_unless_explicit_path_set() {
    let wrapper = dev2merge_workflow_step_bash("Plan with mktd");
    assert!(
        wrapper.contains("${CSA_WORKFLOW_DIR:-patterns/dev2merge}/scripts/csa/plan-with-mktd.sh"),
        "Step 7 must invoke the bundled helper through CSA_WORKFLOW_DIR"
    );
    let script = dev2merge_bundled_helper("plan-with-mktd.sh");

    assert!(
        script.contains("CSA_BIN=\"${CSA_BIN:-csa}\""),
        "Step 7 must default CSA_BIN to csa while honoring the exact parent binary when provided"
    );
    assert!(
        script.contains("MKTD=(--pattern mktd); [ -n \"${MKTD_WORKFLOW_PATH:-}\" ]"),
        "Step 7 must expose an explicit mktd workflow path override while defaulting to pattern resolution"
    );
    assert!(
        script.contains("MKTD=(--pattern mktd)"),
        "Step 7 must default to pattern resolution instead of a target-repo-local path"
    );
    assert!(
        script.contains("MKTD=(\"$MKTD_WORKFLOW_PATH\")"),
        "Step 7 must honor explicit mktd workflow path configuration"
    );
    assert!(
        !script.contains("csa plan run --sa-mode true patterns/mktd/workflow.toml"),
        "Step 7 must not invoke target-repo-local patterns/mktd/workflow.toml by default"
    );
    assert!(
        script.contains(
            "timeout -k 30 \"${MKTD_TIMEOUT_SECONDS}\" \"${CSA_BIN}\" plan run --sa-mode true"
        ),
        "Step 7 must run nested mktd through CSA_BIN so exact-head parents do not fall back to stale PATH csa"
    );
    assert!(
        script.contains("\"${CSA_BIN}\" todo list --format json")
            && script.contains("\"${CSA_BIN}\" todo show -t \"${LATEST_TS}\" --path"),
        "Step 7 must use CSA_BIN for post-mktd TODO discovery too"
    );
    assert!(
        !script.contains("timeout -k 30 \"${MKTD_TIMEOUT_SECONDS}\" csa plan run"),
        "Step 7 must not use bare csa for nested mktd"
    );
}

#[cfg(unix)]
#[test]
fn dev2merge_version_bump_skips_when_optional_check_recipe_absent() {
    let fake_just = r#"
case "${1:-}" in
  --summary)
    printf '%s\n' 'test check-version-bumped-extra'
    ;;
  *)
    printf 'unexpected just invocation: %s\n' "$*" >&2
    exit 97
    ;;
esac
"#;

    for title in ["FAST_PATH Version Bump", "Ensure Version Bumped"] {
        let output = run_version_bump_script_with_fake_just(title, fake_just);
        assert!(
            output.status.success(),
            "{title} must skip cleanly when check-version-bumped is absent: stdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains("Version bump skipped: no-check-version-bumped"),
            "{title} must print a bounded skip message when check-version-bumped is absent"
        );
    }
}

#[cfg(unix)]
#[test]
fn dev2merge_version_bump_uses_exact_optional_recipe_names() {
    let fake_just = r#"
case "${1:-}" in
  --summary)
    printf '%s\n' 'check-version-bumped foo-bump-patch bump-patch-extra'
    ;;
  check-version-bumped)
    exit 1
    ;;
  *)
    printf 'unexpected just invocation: %s\n' "$*" >&2
    exit 97
    ;;
esac
"#;

    for title in ["FAST_PATH Version Bump", "Ensure Version Bumped"] {
        let output = run_version_bump_script_with_fake_just(title, fake_just);
        assert!(
            output.status.success(),
            "{title} must not treat hyphenated recipe substrings as exact recipe names: stdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("Version bump skipped: no-bump-patch"),
            "{title} must skip because exact bump-patch recipe is absent"
        );
    }
}

#[cfg(unix)]
#[test]
fn dev2merge_version_bump_skips_when_optional_bump_recipe_absent() {
    let fake_just = r#"
case "${1:-}" in
  --summary)
    printf '%s\n' 'check-version-bumped'
    ;;
  check-version-bumped)
    exit 1
    ;;
  *)
    printf 'unexpected just invocation: %s\n' "$*" >&2
    exit 97
    ;;
esac
"#;

    for title in ["FAST_PATH Version Bump", "Ensure Version Bumped"] {
        let output = run_version_bump_script_with_fake_just(title, fake_just);
        assert!(
            output.status.success(),
            "{title} must skip cleanly when bump-patch is absent: stdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("Version bump skipped: no-bump-patch"),
            "{title} must print a bounded skip message when bump-patch is absent"
        );
    }
}

#[cfg(unix)]
fn run_version_bump_script_with_fake_just(
    title: &str,
    fake_just_body: &str,
) -> std::process::Output {
    let repo = TempDir::new().unwrap();
    let bin = repo.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let just_path = bin.join("just");
    std::fs::write(
        &just_path,
        format!("#!/usr/bin/env bash\nset -euo pipefail\n{fake_just_body}"),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&just_path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&just_path, permissions).unwrap();
    std::fs::write(
        repo.path().join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();

    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    Command::new("bash")
        .arg("-c")
        .arg(dev2merge_workflow_step_bash(title))
        .current_dir(repo.path())
        .env("PATH", path)
        .output()
        .unwrap()
}
