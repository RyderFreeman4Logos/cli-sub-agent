use crate::test_bounded_command::status_with_timeout;
use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use weave::compiler::{PlanStep, VariableDecl, plan_from_toml};

pub(super) fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

pub(super) fn write_executable(path: &Path, content: &str) {
    std::fs::write(path, content).unwrap();
    #[cfg(unix)]
    {
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }
}

pub(super) fn pr_bot_plan_steps_by_title(titles: &[&str]) -> (Vec<VariableDecl>, Vec<PlanStep>) {
    let workflow_path = workspace_root().join("patterns/pr-bot/workflow.toml");
    let workflow = std::fs::read_to_string(&workflow_path).unwrap();
    let plan = plan_from_toml(&workflow).unwrap();
    let steps = titles
        .iter()
        .map(|title| {
            plan.steps
                .iter()
                .find(|step| step.title == *title)
                .unwrap_or_else(|| panic!("missing pr-bot step '{title}'"))
                .clone()
        })
        .collect();
    (plan.variables, steps)
}

pub(super) fn install_pr_bot_degraded_gate_stubs(root: &Path) -> PathBuf {
    let bin_dir = root.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let capture_path = root.join("gh-comments.md");

    write_executable(
        &bin_dir.join("git"),
        r#"#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
  rev-parse)
    if [ "${2:-}" = "HEAD" ]; then
      echo "abc123postfixhead"
      exit 0
    fi
    ;;
  diff)
    if [ "${2:-}" = "--stat" ]; then
      printf 'crates/example.rs | 2 ++\n 1 file changed, 2 insertions(+)\n'
      exit 0
    fi
    ;;
  push)
    echo "fake git push $*" >&2
    exit 0
    ;;
esac
echo "unexpected git invocation: $*" >&2
exit 2
"#,
    );

    write_executable(
        &bin_dir.join("csa"),
        r#"#!/usr/bin/env bash
set -euo pipefail
if [ "${1:-}" = "run" ]; then
  echo "01ARZ3NDEKTSV4RRFFQ69G5FAV"
  exit 0
fi
if [ "${1:-}" = "review" ]; then
  case "${TEST_LOCAL_REVIEW_MODE:-}" in
    degraded)
      echo "Review verdict: UNAVAILABLE"
      echo "tool unavailable: codex auth required"
      exit 1
      ;;
    *)
      echo "unexpected local review invocation" >&2
      exit 99
      ;;
  esac
fi
echo "unexpected csa invocation: $*" >&2
exit 2
"#,
    );

    write_executable(
        &bin_dir.join("gh"),
        r#"#!/usr/bin/env bash
set -euo pipefail
if [ "${1:-}" = "api" ]; then
  url="${*: -1}"
  case "${url}" in
    */issues/*/comments?per_page=100)
      echo '[[]]'
      exit 0
      ;;
    */pulls/*/reviews?per_page=100)
      echo '[[]]'
      exit 0
      ;;
    */pulls/*/comments*)
      case "${TEST_CLOUD_FINDING:-medium}" in
        medium)
          echo '[[{"user":{"login":"codex"},"created_at":"9999-01-01T00:00:00Z","body":"P2 medium: non-blocking follow-up"}]]'
          ;;
        high)
          echo '[[{"user":{"login":"codex"},"created_at":"9999-01-01T00:00:00Z","body":"HIGH: blocking correctness issue"}]]'
          ;;
        none)
          echo '[[]]'
          ;;
        *)
          echo "unknown TEST_CLOUD_FINDING=${TEST_CLOUD_FINDING}" >&2
          exit 2
          ;;
      esac
      exit 0
      ;;
  esac
fi

if [ "${1:-}" = "pr" ] && [ "${2:-}" = "comment" ]; then
  body=""
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --body)
        body="$2"
        shift 2
        ;;
      *)
        shift
        ;;
    esac
  done
  printf '%s\n---COMMENT---\n' "${body}" >> "${TEST_GH_CAPTURE:?missing TEST_GH_CAPTURE}"
  exit 0
fi

echo "unexpected gh invocation: $*" >&2
exit 2
"#,
    );

    let helper_dir = root.join("scripts/csa");
    std::fs::create_dir_all(&helper_dir).unwrap();
    write_executable(
        &helper_dir.join("session-wait-until-done.sh"),
        r#"#!/usr/bin/env bash
set -euo pipefail
echo "BOT_REPLY=received"
"#,
    );
    // CSA_WORKFLOW_DIR resolves to the temp project root for execute_plan helpers.
    // Step 10b probes native-review-bypass before launching local review; install the
    // real helper so the missing-script path cannot inject ambient stderr noise under
    // the full Static partition (#2847).
    std::fs::copy(
        workspace_root().join("patterns/pr-bot/scripts/csa/native-review-bypass.sh"),
        helper_dir.join("native-review-bypass.sh"),
    )
    .unwrap();

    capture_path
}

pub(super) fn pr_bot_degraded_gate_vars(
    capture_path: &Path,
    finding: &str,
    local_review: &str,
) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    let bin_dir = capture_path.parent().unwrap().join("bin");
    let existing_path = std::env::var("PATH").unwrap_or_default();
    vars.insert(
        "PATH".into(),
        format!("{}:{}", bin_dir.display(), existing_path),
    );
    vars.insert("BOT_HAS_ISSUES".into(), "true".into());
    vars.insert("BOT_UNAVAILABLE".into(), "false".into());
    vars.insert("CLOUD_BOT".into(), "true".into());
    vars.insert("CLOUD_BOT_LOGIN".into(), "codex".into());
    vars.insert("CLOUD_BOT_NAME".into(), "codex".into());
    vars.insert("CLOUD_BOT_POLL_MAX_SECONDS".into(), "1".into());
    vars.insert("CLOUD_BOT_RETRIGGER_CMD".into(), "@codex review".into());
    vars.insert("CLOUD_BOT_WAIT_SECONDS".into(), "0".into());
    vars.insert("DEFAULT_BRANCH".into(), "main".into());
    vars.insert("FALLBACK_REVIEW_HAS_ISSUES".into(), "false".into());
    vars.insert("FIXES_ACCUMULATED".into(), "false".into());
    vars.insert("MERGE_COMPLETED".into(), "false".into());
    vars.insert("POLL_IDLE_TIMEOUT".into(), "1".into());
    vars.insert("POLL_MAX_TIMEOUT".into(), "1".into());
    vars.insert("PR_NUM".into(), "1788".into());
    vars.insert("REBASE_CLEAN_HISTORY_APPLIED".into(), "false".into());
    vars.insert("REBASE_REVIEW_HAS_ISSUES".into(), "false".into());
    vars.insert("REMOTE_NAME".into(), "origin".into());
    vars.insert("REPO".into(), "RyderFreeman4Logos/cli-sub-agent".into());
    vars.insert("ROUND_LIMIT_REACHED".into(), "false".into());
    vars.insert("WORKFLOW_BRANCH".into(), "fix/1788".into());
    vars.insert("BOT_SETTLE_SECS".into(), "0".into());
    vars.insert("TEST_CLOUD_FINDING".into(), finding.into());
    vars.insert("TEST_GH_CAPTURE".into(), capture_path.display().to_string());
    vars.insert("TEST_LOCAL_REVIEW_MODE".into(), local_review.into());
    vars
}

pub(super) fn install_pr_bot_local_review_stubs(root: &Path, current_head: &str) -> PathBuf {
    let bin_dir = root.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let csa_called_path = root.join("csa-review-called");

    write_executable(
        &bin_dir.join("git"),
        &format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
if [ "${{1:-}}" = "rev-parse" ] && [ "${{2:-}}" = "HEAD" ]; then
  echo "{current_head}"
  exit 0
fi
if [ "${{1:-}}" = "config" ] && [ "${{2:-}}" = "user.email" ]; then
  echo "reviewer@example.com"
  exit 0
fi
echo "unexpected git invocation: $*" >&2
exit 2
"#
        ),
    );

    write_executable(
        &bin_dir.join("csa"),
        r#"#!/usr/bin/env bash
set -euo pipefail
if [ "${1:-}" = "config" ] && [ "${2:-}" = "get" ]; then
  key="${3:-}"
  default=""
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --default)
        default="${2:-}"
        shift 2
        ;;
      *)
        shift
        ;;
    esac
  done
  if [ "${key}" = "pr_review.cloud_bot" ]; then
    echo "${TEST_CLOUD_BOT:-true}"
  else
    echo "${default}"
  fi
  exit 0
fi
if [ "${1:-}" = "run" ]; then
  echo "01ARZ3NDEKTSV4RRFFQ69G5FAV"
  exit 0
fi
if [ "${1:-}" = "review" ]; then
  touch "${TEST_CSA_REVIEW_CALLED:?missing TEST_CSA_REVIEW_CALLED}"
  echo "CSA review should only run when no bounded native bypass evidence matches" >&2
  exit 42
fi
echo "unexpected csa invocation: $*" >&2
exit 2
"#,
    );

    write_executable(
        &bin_dir.join("gh"),
        r#"#!/usr/bin/env bash
set -euo pipefail
if [ "${1:-}" = "api" ]; then
  echo '[[]]'
  exit 0
fi
if [ "${1:-}" = "pr" ] && [ "${2:-}" = "comment" ]; then
  exit 0
fi
echo "unexpected gh invocation: $*" >&2
exit 2
"#,
    );

    let helper_dir = root.join("scripts/csa");
    std::fs::create_dir_all(&helper_dir).unwrap();
    write_executable(
        &helper_dir.join("latest-pass-review-head.sh"),
        r#"#!/usr/bin/env bash
set -euo pipefail
exit 0
"#,
    );
    write_executable(
        &helper_dir.join("session-wait-until-done.sh"),
        r#"#!/usr/bin/env bash
set -euo pipefail
if [ "${TEST_SESSION_WAIT_TIMEOUT:-false}" = "true" ]; then
  echo "BOT_REPLY=timeout"
  exit 0
fi
echo "unexpected session wait for $*" >&2
exit 42
"#,
    );
    std::fs::copy(
        workspace_root().join("patterns/pr-bot/scripts/csa/native-review-bypass.sh"),
        helper_dir.join("native-review-bypass.sh"),
    )
    .unwrap();
    let script_dir = root.join("scripts");
    std::fs::create_dir_all(&script_dir).unwrap();
    std::fs::copy(
        workspace_root().join("patterns/pr-bot/scripts/pr-bot-quota-cache.sh"),
        script_dir.join("pr-bot-quota-cache.sh"),
    )
    .unwrap();

    csa_called_path
}

pub(super) fn write_native_review_bypass_artifact(root: &Path, head: &str) {
    let artifact_dir = root.join(".csa/native-review-bypass");
    std::fs::create_dir_all(&artifact_dir).unwrap();
    std::fs::write(
        artifact_dir.join(format!("{head}.toml")),
        format!(
            "schema_version=1\nartifact_kind=\"native_review_bypass\"\nsource=\"native\"\nhead_sha=\"{head}\"\nrange=\"main...HEAD\"\nverdict=\"clean\"\n"
        ),
    )
    .unwrap();
}

pub(super) fn write_review_check_skip_audit_log(root: &Path) {
    let bin_dir = root.join("bin");
    let existing_path = std::env::var("PATH").unwrap_or_default();
    let status = status_with_timeout(
        {
            let mut command = Command::new("bash");
            command
                .arg(workspace_root().join("scripts/hooks/review-check.sh"))
                .current_dir(root)
                .env("PATH", format!("{}:{}", bin_dir.display(), existing_path))
                .env("CSA_SKIP_REVIEW_CHECK", "1")
                .env(
                    "CSA_SKIP_REVIEW_CHECK_REASON",
                    "source=native range=main...HEAD verdict=clean",
                );
            command
        },
        Duration::from_secs(30),
    );
    assert!(status.success());
}

pub(super) fn pr_bot_local_review_vars(
    root: &Path,
    csa_called_path: &Path,
) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    let bin_dir = root.join("bin");
    let existing_path = std::env::var("PATH").unwrap_or_default();
    vars.insert(
        "PATH".into(),
        format!("{}:{}", bin_dir.display(), existing_path),
    );
    vars.insert("CSA_WORKFLOW_DIR".into(), root.display().to_string());
    vars.insert("DEFAULT_BRANCH".into(), "main".into());
    vars.insert(
        "TEST_CSA_REVIEW_CALLED".into(),
        csa_called_path.display().to_string(),
    );
    vars
}
