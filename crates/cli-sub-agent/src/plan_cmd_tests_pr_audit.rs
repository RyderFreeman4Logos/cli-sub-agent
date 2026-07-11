fn install_fake_gh(bin_dir: &Path) -> PathBuf {
    let capture_path = bin_dir.join("gh-capture.md");
    let gh_path = bin_dir.join("gh");
    std::fs::write(
        &gh_path,
        r#"#!/usr/bin/env bash
set -euo pipefail
body_file=""
capture="${TEST_GH_CAPTURE:?missing TEST_GH_CAPTURE}"
while [ $# -gt 0 ]; do
  case "$1" in
    --body-file)
      body_file="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
if [ -z "${body_file}" ]; then
  echo "missing --body-file" >&2
  exit 1
fi
cp "${body_file}" "${capture}"
"#,
    )
    .unwrap();
    #[cfg(unix)]
    {
        let mut perms = std::fs::metadata(&gh_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&gh_path, perms).unwrap();
    }
    capture_path
}

fn step_15_env(
    bin_dir: &Path,
    capture_path: &Path,
    step_12_output: &str,
) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    let existing_path = std::env::var("PATH").unwrap_or_default();
    vars.insert("STEP_12_OUTPUT".into(), step_12_output.into());
    vars.insert("PR_NUM".into(), "357".into());
    vars.insert("REPO".into(), "RyderFreeman4Logos/cli-sub-agent".into());
    vars.insert("BOT_UNAVAILABLE".into(), "false".into());
    vars.insert("BOT_HAS_ISSUES".into(), "true".into());
    vars.insert("COMMENT_IS_FALSE_POSITIVE".into(), "true".into());
    vars.insert("COMMENT_IS_STALE".into(), "false".into());
    vars.insert(
        "PATH".into(),
        format!("{}:{}", bin_dir.display(), existing_path),
    );
    vars.insert("TEST_GH_CAPTURE".into(), capture_path.display().to_string());
    vars
}

fn dismissed_debate_output() -> &'static str {
    r#"VERDICT: DISMISSED
RATIONALE: The bot misread the workflow and the fix path still runs for confirmed issues.
PR_COMMENT_START
**Local arbitration result: DISMISSED.**

## Participants
- **Author**: codex/openai/gpt-5/xhigh
- **Arbiter**: gemini-cli/google/default/xhigh

## Bot Concern
The bot warned that the workflow could skip real fixes after arbitration.

## Debate Summary
### Round 1
- **Proposer** (`codex/openai/gpt-5/xhigh`): The new reroute step preserves the fix path.
- **Critic** (`gemini-cli/google/default/xhigh`): The parser must fail closed on malformed markers.

## Conclusion
The finding is dismissed because the workflow now reroutes CONFIRMED verdicts back into the fix step and fails closed on malformed structured output.

CSA session ID: 01TESTDEBATESESSIONID
PR_COMMENT_END
"#
}

fn confirmed_debate_output() -> &'static str {
    r#"VERDICT: CONFIRMED
RATIONALE: The bot concern is valid and this comment must reroute to the fix step.
PR_COMMENT_START
Workflow should not post this text because the verdict is CONFIRMED.
PR_COMMENT_END
"#
}

#[tokio::test]
async fn execute_step_bash_posts_pr_audit_trail_for_dismissed_verdict() {
    let step = load_pr_bot_step_by_title("Step 8a: Post Debate Audit Trail Comment");
    let tmp = tempfile::tempdir().unwrap();
    let capture_path = install_fake_gh(tmp.path());
    let vars = step_15_env(tmp.path(), &capture_path, dismissed_debate_output());

    let result = execute_step(&step, &vars, tmp.path(), None, None, None).await;

    assert_eq!(
        result.exit_code, 0,
        "error={:?} output={:?}",
        result.error, result.output
    );
    assert!(
        result
            .output
            .as_deref()
            .unwrap_or("")
            .contains("CSA_VAR:AUDIT_TRAIL_POSTED=true")
    );

    let comment = std::fs::read_to_string(&capture_path).unwrap();
    assert!(comment.contains("## Participants"));
    assert!(comment.contains("## Bot Concern"));
    assert!(comment.contains("## Debate Summary"));
    assert!(comment.contains("## Conclusion"));
    assert!(comment.contains("CSA session ID: 01TESTDEBATESESSIONID"));
}

#[tokio::test]
async fn execute_step_bash_dedupes_identical_verdict_markers() {
    let step = load_pr_bot_step_by_title("Step 8a: Post Debate Audit Trail Comment");
    let tmp = tempfile::tempdir().unwrap();
    let capture_path = install_fake_gh(tmp.path());
    let duplicate_verdict_output = r#"VERDICT: DISMISSED
RATIONALE: The first marker is duplicated by a later identical marker.
VERDICT: DISMISSED
PR_COMMENT_START
**Local arbitration result: DISMISSED.**

## Participants
- **Author**: codex/openai/gpt-5/xhigh
- **Arbiter**: gemini-cli/google/default/xhigh

## Bot Concern
Identical verdict markers are noisy but not ambiguous.

## Debate Summary
### Round 1
- **Proposer** (`codex/openai/gpt-5/xhigh`): Duplicate identical verdicts should collapse.
- **Critic** (`gemini-cli/google/default/xhigh`): Conflicting verdicts must still fail closed.

## Conclusion
The repeated identical verdict is treated as one DISMISSED verdict.

CSA session ID: 01TESTDEBATESESSIONID
PR_COMMENT_END
"#;
    let vars = step_15_env(tmp.path(), &capture_path, duplicate_verdict_output);

    let result = execute_step(&step, &vars, tmp.path(), None, None, None).await;

    assert_eq!(
        result.exit_code, 0,
        "error={:?} output={:?}",
        result.error, result.output
    );
    assert!(
        result
            .output
            .as_deref()
            .unwrap_or("")
            .contains("CSA_VAR:AUDIT_TRAIL_POSTED=true")
    );

    let comment = std::fs::read_to_string(&capture_path).unwrap();
    assert!(comment.contains("Identical verdict markers are noisy but not ambiguous."));
    assert!(comment.contains("CSA session ID: 01TESTDEBATESESSIONID"));
}

#[tokio::test]
async fn execute_step_bash_reroutes_confirmed_verdict_without_posting_comment() {
    let step = load_pr_bot_step_by_title("Step 8a: Post Debate Audit Trail Comment");
    let tmp = tempfile::tempdir().unwrap();
    let capture_path = install_fake_gh(tmp.path());
    let vars = step_15_env(tmp.path(), &capture_path, confirmed_debate_output());

    let result = execute_step(&step, &vars, tmp.path(), None, None, None).await;

    assert_eq!(
        result.exit_code, 0,
        "error={:?} output={:?}",
        result.error, result.output
    );
    assert!(
        result
            .output
            .as_deref()
            .unwrap_or("")
            .contains("CSA_VAR:AUDIT_TRAIL_POSTED=false")
    );
    assert!(
        result
            .output
            .as_deref()
            .unwrap_or("")
            .contains("CSA_VAR:COMMENT_IS_FALSE_POSITIVE=false")
    );
    assert!(
        !capture_path.exists(),
        "gh pr comment should not run for CONFIRMED verdicts"
    );
}

#[tokio::test]
async fn execute_step_bash_fails_closed_on_malformed_dismissed_output() {
    let step = load_pr_bot_step_by_title("Step 8a: Post Debate Audit Trail Comment");
    let tmp = tempfile::tempdir().unwrap();
    let capture_path = install_fake_gh(tmp.path());
    let malformed_output = r#"VERDICT: DISMISSED
RATIONALE: Missing comment end marker should abort.
PR_COMMENT_START
**Local arbitration result: DISMISSED.**

## Participants
- **Author**: codex/openai/gpt-5/xhigh
- **Arbiter**: gemini-cli/google/default/xhigh

## Bot Concern
Malformed marker contract.

## Debate Summary
### Round 1
- **Proposer** (`codex/openai/gpt-5/xhigh`): Missing end marker.
- **Critic** (`gemini-cli/google/default/xhigh`): The parser should abort.

## Conclusion
Abort rather than post an ambiguous comment.

CSA session ID: 01TESTDEBATESESSIONID
"#;
    let vars = step_15_env(tmp.path(), &capture_path, malformed_output);

    let result = execute_step(&step, &vars, tmp.path(), None, None, None).await;

    assert_ne!(result.exit_code, 0);
    assert!(
        !capture_path.exists(),
        "gh pr comment should not run for malformed debate output"
    );
}

#[tokio::test]
async fn execute_step_bash_fails_closed_on_conflicting_verdict_markers() {
    let step = load_pr_bot_step_by_title("Step 8a: Post Debate Audit Trail Comment");
    let tmp = tempfile::tempdir().unwrap();
    let capture_path = install_fake_gh(tmp.path());
    let duplicate_verdict_output = r#"VERDICT: DISMISSED
RATIONALE: The first verdict is stale.
VERDICT: CONFIRMED
RATIONALE: The final verdict conflicts with the first one.
PR_COMMENT_START
**Local arbitration result: DISMISSED.**

## Participants
- **Author**: codex/openai/gpt-5/xhigh
- **Arbiter**: gemini-cli/google/default/xhigh

## Bot Concern
Conflicting verdict markers must fail closed.

## Debate Summary
### Round 1
- **Proposer** (`codex/openai/gpt-5/xhigh`): Duplicate verdicts are ambiguous.
- **Critic** (`gemini-cli/google/default/xhigh`): The parser should reject them.

## Conclusion
Abort rather than posting an ambiguous dismissal.

CSA session ID: 01TESTDEBATESESSIONID
PR_COMMENT_END
"#;
    let vars = step_15_env(tmp.path(), &capture_path, duplicate_verdict_output);

    let result = execute_step(&step, &vars, tmp.path(), None, None, None).await;

    assert_ne!(result.exit_code, 0);
    assert!(
        !capture_path.exists(),
        "gh pr comment should not run for duplicate verdict markers"
    );
}
