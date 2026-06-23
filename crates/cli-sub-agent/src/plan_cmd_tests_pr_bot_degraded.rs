use super::*;
use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use weave::compiler::{ExecutionPlan, FailAction, PlanStep, VariableDecl, plan_from_toml};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn write_executable(path: &Path, content: &str) {
    std::fs::write(path, content).unwrap();
    #[cfg(unix)]
    {
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }
}

fn pr_bot_plan_steps_by_title(titles: &[&str]) -> (Vec<VariableDecl>, Vec<PlanStep>) {
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

fn install_pr_bot_degraded_gate_stubs(root: &Path) -> PathBuf {
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

    capture_path
}

fn pr_bot_degraded_gate_vars(
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

fn install_pr_bot_local_review_stubs(root: &Path, current_head: &str) -> PathBuf {
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

fn write_native_review_bypass_artifact(root: &Path, head: &str) {
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

fn write_review_check_skip_audit_log(root: &Path) {
    let bin_dir = root.join("bin");
    let existing_path = std::env::var("PATH").unwrap_or_default();
    let status = Command::new("bash")
        .arg(workspace_root().join("scripts/hooks/review-check.sh"))
        .current_dir(root)
        .env("PATH", format!("{}:{}", bin_dir.display(), existing_path))
        .env("CSA_SKIP_REVIEW_CHECK", "1")
        .env(
            "CSA_SKIP_REVIEW_CHECK_REASON",
            "source=native range=main...HEAD verdict=clean",
        )
        .status()
        .unwrap();
    assert!(status.success());
}

fn pr_bot_local_review_vars(root: &Path, csa_called_path: &Path) -> HashMap<String, String> {
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

#[tokio::test]
async fn execute_pr_bot_local_review_accepts_same_sha_native_bypass_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let current_head = "abcdef1234567890abcdef1234567890abcdef12";
    let csa_called_path = install_pr_bot_local_review_stubs(tmp.path(), current_head);
    write_native_review_bypass_artifact(tmp.path(), current_head);

    let vars = pr_bot_local_review_vars(tmp.path(), &csa_called_path);
    let (variables, steps) =
        pr_bot_plan_steps_by_title(&["Local Pre-PR Review (SYNCHRONOUS — MUST NOT background)"]);
    let plan = ExecutionPlan {
        name: "pr-bot-native-review-bypass".into(),
        description: String::new(),
        variables,
        steps,
    };

    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .expect("native fallback evidence should satisfy local review");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].exit_code, 0, "Step 2 should pass");
    let output = results[0].output.as_deref().unwrap_or("");
    assert!(
        output.contains("Fast-path: structured native review bypass artifact covers current HEAD"),
        "expected native bypass fast-path output: {output}"
    );
    assert!(
        output.contains("CSA_VAR:LOCAL_REVIEW_SESSION_ID=native-review-bypass-abcdef123456"),
        "native fallback should publish a bounded synthetic session id: {output}"
    );
    assert!(
        !csa_called_path.exists(),
        "current-head native fallback evidence must avoid an unnecessary CSA review launch"
    );
}

#[tokio::test]
async fn execute_pr_bot_local_review_rejects_stale_native_bypass_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let current_head = "abcdef1234567890abcdef1234567890abcdef12";
    let stale_head = "1234567890abcdef1234567890abcdef12345678";
    let csa_called_path = install_pr_bot_local_review_stubs(tmp.path(), current_head);
    write_native_review_bypass_artifact(tmp.path(), stale_head);

    let vars = pr_bot_local_review_vars(tmp.path(), &csa_called_path);
    let (variables, steps) =
        pr_bot_plan_steps_by_title(&["Local Pre-PR Review (SYNCHRONOUS — MUST NOT background)"]);
    let plan = ExecutionPlan {
        name: "pr-bot-stale-native-review-bypass".into(),
        description: String::new(),
        variables,
        steps,
    };

    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .expect("stale evidence should fail through the CSA review path");

    assert_eq!(results.len(), 1);
    assert_ne!(
        results[0].exit_code, 0,
        "stale bypass evidence must not satisfy local review"
    );
    assert!(
        csa_called_path.exists(),
        "stale fallback evidence should force pr-bot back to CSA review"
    );
}

#[tokio::test]
async fn execute_pr_bot_local_review_rejects_review_check_skip_audit_log() {
    let tmp = tempfile::tempdir().unwrap();
    let current_head = "abcdef1234567890abcdef1234567890abcdef12";
    let csa_called_path = install_pr_bot_local_review_stubs(tmp.path(), current_head);
    write_review_check_skip_audit_log(tmp.path());

    let vars = pr_bot_local_review_vars(tmp.path(), &csa_called_path);
    let (variables, steps) =
        pr_bot_plan_steps_by_title(&["Local Pre-PR Review (SYNCHRONOUS — MUST NOT background)"]);
    let plan = ExecutionPlan {
        name: "pr-bot-skip-log-rejected".into(),
        description: String::new(),
        variables,
        steps,
    };

    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .expect("hook skip log should fall through");

    assert_eq!(results.len(), 1);
    assert_ne!(results[0].exit_code, 0, "hook skip log must fail");
    assert!(csa_called_path.exists(), "should force CSA review");
    let stderr = results[0].stderr.as_deref().unwrap_or("");
    assert!(stderr.contains("audit-only") || stderr.contains("Audit-only"));
}

#[tokio::test]
async fn execute_pr_bot_bot_unavailable_gate_accepts_native_bypass_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let current_head = "abcdef1234567890abcdef1234567890abcdef12";
    let csa_called_path = install_pr_bot_local_review_stubs(tmp.path(), current_head);
    write_native_review_bypass_artifact(tmp.path(), current_head);

    let mut vars = pr_bot_local_review_vars(tmp.path(), &csa_called_path);
    vars.insert("MERGE_COMPLETED".into(), "false".into());
    vars.insert("TEST_CLOUD_BOT".into(), "false".into());
    vars.insert("PR_NUM".into(), "1788".into());
    vars.insert("REPO".into(), "RyderFreeman4Logos/cli-sub-agent".into());
    let (variables, steps) =
        pr_bot_plan_steps_by_title(&["Step 4a: Check Cloud Bot Configuration"]);
    let plan = ExecutionPlan {
        name: "pr-bot-bot-unavailable-native-review-bypass".into(),
        description: String::new(),
        variables,
        steps,
    };

    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .expect("bot-unavailable native fallback evidence should satisfy local review");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].exit_code, 0, "Step 4a should pass");
    let output = results[0].output.as_deref().unwrap_or("");
    assert!(
        output.contains("Merge-without-bot native review bypass covers HEAD"),
        "expected bot-unavailable bypass output: {output}"
    );
    assert!(
        output.contains("CSA_VAR:LOCAL_REVIEW_SESSION_ID=native-review-bypass-abcdef123456"),
        "native fallback should publish a bounded synthetic session id: {output}"
    );
    assert!(
        !csa_called_path.exists(),
        "bot-unavailable native fallback evidence must avoid an unnecessary CSA review launch"
    );
}

#[tokio::test]
async fn execute_pr_bot_post_fix_fallback_accepts_native_bypass_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let current_head = "abcdef1234567890abcdef1234567890abcdef12";
    let csa_called_path = install_pr_bot_local_review_stubs(tmp.path(), current_head);
    write_native_review_bypass_artifact(tmp.path(), current_head);

    let mut vars = pr_bot_local_review_vars(tmp.path(), &csa_called_path);
    vars.insert("BOT_HAS_ISSUES".into(), "true".into());
    vars.insert("BOT_SETTLE_SECS".into(), "0".into());
    vars.insert("BOT_UNAVAILABLE".into(), "false".into());
    vars.insert("CLOUD_BOT".into(), "true".into());
    vars.insert("CLOUD_BOT_LOGIN".into(), "codex".into());
    vars.insert("CLOUD_BOT_NAME".into(), "codex".into());
    vars.insert("CLOUD_BOT_POLL_MAX_SECONDS".into(), "1".into());
    vars.insert("CLOUD_BOT_RETRIGGER_CMD".into(), "@codex review".into());
    vars.insert("CLOUD_BOT_WAIT_SECONDS".into(), "0".into());
    vars.insert("FALLBACK_REVIEW_HAS_ISSUES".into(), "false".into());
    vars.insert("MERGE_COMPLETED".into(), "false".into());
    vars.insert("POLL_IDLE_TIMEOUT".into(), "1800".into());
    vars.insert("POLL_MAX_TIMEOUT".into(), "1800".into());
    vars.insert("PR_NUM".into(), "1788".into());
    vars.insert("REPO".into(), "RyderFreeman4Logos/cli-sub-agent".into());
    vars.insert("ROUND_LIMIT_REACHED".into(), "false".into());
    vars.insert("TEST_SESSION_WAIT_TIMEOUT".into(), "true".into());
    let (variables, steps) =
        pr_bot_plan_steps_by_title(&["Step 10b: Post-Fix Re-Review Gate (HARD GATE)"]);
    let plan = ExecutionPlan {
        name: "pr-bot-post-fix-native-review-bypass".into(),
        description: String::new(),
        variables,
        steps,
    };

    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .expect("post-fix native fallback evidence should satisfy local review");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].exit_code, 0, "Step 10b should pass");
    let output = results[0].output.as_deref().unwrap_or("");
    assert!(
        output.contains("Local fallback native review bypass covers HEAD"),
        "expected post-fix fallback bypass output: {output}"
    );
    assert!(
        output.contains("CSA_VAR:BOT_HAS_ISSUES=false"),
        "post-fix fallback should clear bot issue state after native review evidence: {output}"
    );
    assert!(
        output.contains("CSA_VAR:LOCAL_REVIEW_SESSION_ID=native-review-bypass-abcdef123456"),
        "native fallback should publish a bounded synthetic session id: {output}"
    );
    assert!(
        !csa_called_path.exists(),
        "post-fix native fallback evidence must avoid an unnecessary CSA review launch"
    );
}

#[tokio::test]
async fn execute_pr_bot_degraded_local_fallback_records_rationale_and_reaches_merge() {
    let tmp = tempfile::tempdir().unwrap();
    let capture_path = install_pr_bot_degraded_gate_stubs(tmp.path());
    let vars = pr_bot_degraded_gate_vars(&capture_path, "medium", "degraded");
    let (variables, steps) = pr_bot_plan_steps_by_title(&[
        "Step 10b: Post-Fix Re-Review Gate (HARD GATE)",
        "Step 6a: Merge Without Bot",
        "Step 12b: Final Merge (Direct or Post-Rebase)",
    ]);
    let final_merge_condition = steps[2].condition.clone();
    let plan = ExecutionPlan {
        name: "pr-bot-degraded-local-fallback".into(),
        description: String::new(),
        variables,
        steps: vec![
            steps[0].clone(),
            steps[1].clone(),
            PlanStep {
                id: 99,
                title: "final merge condition reached".into(),
                tool: Some("bash".into()),
                prompt: "```bash\ntouch reached-final-merge\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: final_merge_condition,
                loop_var: None,
                session: None,
                workspace_access: None,
            },
        ],
    };

    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .expect("degraded fallback plan should execute");

    assert_eq!(results.len(), 3, "all gate steps should execute");
    assert!(
        results.iter().all(|result| !result.skipped),
        "no step should be skipped: {:?}",
        results
            .iter()
            .map(|r| (&r.title, r.skipped))
            .collect::<Vec<_>>()
    );
    assert!(
        results.iter().all(|result| result.exit_code == 0),
        "all steps should pass: {:?}",
        results
            .iter()
            .map(|r| (&r.title, r.exit_code, &r.error, &r.stderr))
            .collect::<Vec<_>>()
    );
    let gate_output = results[0].output.as_deref().unwrap_or("");
    assert!(gate_output.contains("CSA_VAR:BOT_UNAVAILABLE=true"));
    assert!(gate_output.contains("CSA_VAR:FALLBACK_REVIEW_HAS_ISSUES=false"));
    assert!(gate_output.contains("CSA_VAR:BOT_HAS_ISSUES=false"));
    assert!(
        gate_output.contains(
            "CSA_VAR:MERGE_WITHOUT_BOT_REASON_KIND=local_review_degraded_no_blocking_findings"
        ),
        "post-fix gate must publish the degraded-review rationale key"
    );
    let rationale_output = results[1].output.as_deref().unwrap_or("");
    assert!(
        rationale_output.contains("merge rationale recorded; proceed to Final Merge"),
        "Step 6a should emit the proceed directive"
    );
    assert!(
        tmp.path().join("reached-final-merge").exists(),
        "final merge condition should be reachable after Step 6a"
    );

    let comments = std::fs::read_to_string(&capture_path).unwrap();
    assert!(comments.contains("local fallback review degraded"));
    assert!(
        comments.contains("MEDIUM/P2 comments are non-blocking follow-up by policy"),
        "rationale must compose with MEDIUM/P2 non-blocking policy"
    );
    assert!(
        comments.contains("Local fallback review could not complete with an available reviewer"),
        "rationale should explain the degraded local fallback"
    );
}

#[tokio::test]
async fn execute_pr_bot_degraded_local_fallback_still_blocks_high_findings() {
    let tmp = tempfile::tempdir().unwrap();
    let capture_path = install_pr_bot_degraded_gate_stubs(tmp.path());
    let vars = pr_bot_degraded_gate_vars(&capture_path, "high", "degraded");
    let (variables, steps) = pr_bot_plan_steps_by_title(&[
        "Step 10b: Post-Fix Re-Review Gate (HARD GATE)",
        "Step 6a: Merge Without Bot",
    ]);
    let plan = ExecutionPlan {
        name: "pr-bot-high-finding-still-blocks".into(),
        description: String::new(),
        variables,
        steps,
    };

    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .expect("plan should return failed step result");

    assert_eq!(
        results.len(),
        1,
        "Step 6a must not run after blocking finding"
    );
    assert_ne!(results[0].exit_code, 0, "blocking HIGH finding must abort");
    assert!(
        results[0]
            .stderr
            .as_deref()
            .unwrap_or("")
            .contains("Post-fix re-review found 1 new blocking finding"),
        "stderr should report the blocking bot finding"
    );
    assert!(
        !tmp.path().join("reached-final-merge").exists(),
        "merge path must remain unreachable"
    );
    let comments = std::fs::read_to_string(&capture_path).unwrap_or_default();
    assert!(
        !comments.contains("local fallback review degraded"),
        "blocking findings must not get a degraded-review merge rationale"
    );
}
