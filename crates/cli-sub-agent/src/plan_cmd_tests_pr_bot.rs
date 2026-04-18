use super::plan_cmd_steps::execute_step_with_workflow;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use weave::compiler::{FailAction, PlanStep, plan_from_toml};

#[tokio::test]
async fn execute_step_with_workflow_exposes_runtime_paths_to_bash() {
    let project_root = tempfile::tempdir().unwrap();
    let workflow_home = tempfile::tempdir().unwrap();
    let workflow_path = workflow_home.path().join("workflow.toml");
    std::fs::write(&workflow_path, "[workflow]\nname='runtime-env'\n").unwrap();

    let step = PlanStep {
        id: 1,
        title: "runtime env".into(),
        tool: Some("bash".into()),
        prompt: "```bash\nprintf '%s\\n%s\\n%s\\n' \"$CSA_PROJECT_ROOT\" \"$CSA_WORKFLOW_PATH\" \"$CSA_WORKFLOW_DIR\" > runtime-env.txt\n```".into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
        session: None,
    };
    let vars = HashMap::new();

    let result = execute_step_with_workflow(
        &step,
        &vars,
        project_root.path(),
        &workflow_path,
        None,
        None,
    )
    .await;
    assert_eq!(result.exit_code, 0, "bash step should succeed");

    let env_dump = std::fs::read_to_string(project_root.path().join("runtime-env.txt")).unwrap();
    let mut lines = env_dump.lines();
    assert_eq!(
        Path::new(lines.next().expect("missing project root env")),
        project_root.path()
    );
    assert_eq!(
        Path::new(lines.next().expect("missing workflow path env")),
        workflow_path.as_path()
    );
    assert_eq!(
        Path::new(lines.next().expect("missing workflow dir env")),
        workflow_home.path()
    );
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn pr_bot_artifact_text(path: &str) -> String {
    std::fs::read_to_string(workspace_root().join(path)).unwrap()
}

fn assert_marker_order(text: &str, first: &str, second: &str, artifact: &str) {
    let first_idx = text
        .find(first)
        .unwrap_or_else(|| panic!("{artifact} must contain marker '{first}'"));
    let second_idx = text
        .find(second)
        .unwrap_or_else(|| panic!("{artifact} must contain marker '{second}'"));
    assert!(
        first_idx < second_idx,
        "{artifact} must place '{first}' before '{second}'"
    );
}

fn extract_nth_shell_function(text: &str, name: &str, occurrence: usize, artifact: &str) -> String {
    let header = format!("{name}() {{");
    let start = text
        .match_indices(&header)
        .nth(occurrence)
        .map(|(idx, _)| idx)
        .unwrap_or_else(|| {
            panic!("{artifact} must contain function '{name}' occurrence {occurrence}")
        });
    let body = &text[start..];
    let end = body.find("\n}\n").unwrap_or_else(|| {
        panic!("{artifact} function '{name}' occurrence {occurrence} must terminate")
    });
    body[..end + 3].to_string()
}

fn git_archive_entries(repo_root: &Path, pathspec: &str) -> Vec<String> {
    let tree = Command::new("git")
        .args(["write-tree"])
        .current_dir(repo_root)
        .output()
        .expect("git write-tree should run");
    assert!(
        tree.status.success(),
        "git write-tree failed: {}",
        String::from_utf8_lossy(&tree.stderr)
    );
    let tree_id = String::from_utf8(tree.stdout)
        .expect("tree id should be utf-8")
        .trim()
        .to_string();

    let archive = Command::new("git")
        .args(["archive", "--format=tar", &tree_id, pathspec])
        .current_dir(repo_root)
        .output()
        .expect("git archive should run");
    assert!(
        archive.status.success(),
        "git archive failed: {}",
        String::from_utf8_lossy(&archive.stderr)
    );

    let mut tar = Command::new("tar")
        .args(["tf", "-"])
        .current_dir(repo_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("tar should start");
    tar.stdin
        .as_mut()
        .expect("tar stdin")
        .write_all(&archive.stdout)
        .expect("should stream archive into tar");
    let listing = tar.wait_with_output().expect("tar should finish");
    assert!(
        listing.status.success(),
        "tar listing failed: {}",
        String::from_utf8_lossy(&listing.stderr)
    );
    String::from_utf8(listing.stdout)
        .expect("tar output should be utf-8")
        .lines()
        .map(ToOwned::to_owned)
        .collect()
}

#[test]
fn pr_bot_workflow_is_v1_loop_free() {
    let workflow_path = workspace_root().join("patterns/pr-bot/workflow.toml");
    let workflow = std::fs::read_to_string(&workflow_path).unwrap();
    let plan = plan_from_toml(&workflow).unwrap();

    let loop_steps: Vec<usize> = plan
        .steps
        .iter()
        .filter_map(|step| step.loop_var.as_ref().map(|_| step.id))
        .collect();

    assert!(
        loop_steps.is_empty(),
        "pr-bot must remain v1-compatible; loop_var found on steps {loop_steps:?}"
    );
}

#[test]
fn pr_bot_workflow_resolves_helpers_from_pattern_dir() {
    let workflow_path = workspace_root().join("patterns/pr-bot/workflow.toml");
    let workflow = std::fs::read_to_string(&workflow_path).unwrap();

    assert!(
        workflow.contains("CSA_HELPER_DIR=\"${CSA_WORKFLOW_DIR}/scripts/csa\""),
        "pr-bot must resolve bundled helpers from the workflow directory"
    );
    assert!(
        !workflow.contains("bash scripts/csa/"),
        "pr-bot must not depend on the target repo's scripts/ directory"
    );
}

#[test]
fn pr_bot_archive_includes_helper_scripts() {
    let entries = git_archive_entries(&workspace_root(), "patterns/pr-bot");

    assert!(
        entries.contains(&"patterns/pr-bot/scripts/csa/latest-pass-review-head.sh".to_string()),
        "git archive for patterns/pr-bot must include latest-pass-review-head.sh"
    );
    assert!(
        entries.contains(&"patterns/pr-bot/scripts/csa/session-wait-until-done.sh".to_string()),
        "git archive for patterns/pr-bot must include session-wait-until-done.sh"
    );
}

#[test]
fn pr_bot_pattern_and_workflow_reuse_existing_current_head_reviews() {
    for artifact in [
        "patterns/pr-bot/workflow.toml",
        "patterns/pr-bot/PATTERN.md",
    ] {
        let text = pr_bot_artifact_text(artifact);
        assert!(
            text.contains("query_reusable_current_head_review_record"),
            "{artifact} must select a reusable current-HEAD review object"
        );
        assert!(
            text.contains("query_latest_current_head_trigger_ts"),
            "{artifact} must anchor reusable null-commit reviews to a prior current-HEAD trigger"
        );
        assert!(
            text.contains("query_current_window_current_head_review_ts"),
            "{artifact} must probe separately for current-window HEAD reviews"
        );
        assert!(
            text.contains(
                "Reusable @${CLOUD_BOT_NAME} review #${BOT_REUSED_REVIEW_ID} already exists for HEAD"
            ),
            "{artifact} must document reusable current-HEAD review-id reuse"
        );
        assert!(
            text.contains("select(.submitted_at >= \"'\"${WAIT_BASE_TS}\"'\")")
                || text.contains("select(.submitted_at >= \"'\"${RETRIGGER_TS}\"'\")"),
            "{artifact} must gate new current-HEAD review reuse to the active trigger window"
        );
        assert!(
            text.contains("reviews/${BOT_REUSED_REVIEW_ID}/comments?per_page=100"),
            "{artifact} must scope reused target-review comments to BOT_REUSED_REVIEW_ID"
        );
        assert!(
            text.contains("reviews/${POST_FIX_REUSED_REVIEW_ID}/comments?per_page=100"),
            "{artifact} must scope reused post-fix comments to POST_FIX_REUSED_REVIEW_ID"
        );
        assert!(
            text.contains("case \"${BOT_HAS_ISSUES_SOURCE:-current_window_comments}\" in"),
            "{artifact} must branch comment selection by BOT_HAS_ISSUES_SOURCE"
        );
        assert!(
            text.contains("BOT_HAS_ISSUES_SOURCE=\"reused_review_comments\""),
            "{artifact} must record when issues came from a reused review"
        );
        assert!(
            text.contains("current_sha_comments)"),
            "{artifact} must preserve current-SHA fallback comment selection for reused non-target bot findings"
        );
        assert!(
            text.contains("CSA_VAR:BOT_REUSED_REVIEW_ID=${BOT_REUSED_REVIEW_ID}"),
            "{artifact} must persist the reused review id for later steps"
        );
        assert!(
            text.contains("CSA_VAR:BOT_HAS_ISSUES_SOURCE=${BOT_HAS_ISSUES_SOURCE}"),
            "{artifact} must persist the issue-source selector for later steps"
        );
        assert_marker_order(
            &text,
            "# --- Detect whether current HEAD already has a reusable bot review ---",
            "# --- Trigger cloud bot review for current HEAD ---",
            artifact,
        );
        assert_marker_order(
            &text,
            "# --- Detect whether current HEAD already has a reusable post-fix review ---",
            "# --- Re-trigger bot review (ALWAYS explicit — bots don't auto-review on force-push) ---",
            artifact,
        );
    }
}

#[test]
fn pr_bot_artifacts_paginate_current_head_trigger_lookup() {
    for artifact in [
        "patterns/pr-bot/workflow.toml",
        "patterns/pr-bot/PATTERN.md",
    ] {
        let text = pr_bot_artifact_text(artifact);
        for occurrence in 0..2 {
            let helper = extract_nth_shell_function(
                &text,
                "query_latest_current_head_trigger_ts",
                occurrence,
                artifact,
            );
            assert!(
                helper.contains(
                    r#"gh api --paginate "repos/${REPO}/issues/${PR_NUM}/comments?per_page=100" 2>/dev/null"#,
                ),
                "{artifact} helper occurrence {occurrence} must paginate issue comments before piping to jq"
            );
            assert!(
                helper.contains(r#"| jq -s '[.[][] | select((.body // "") | test("csa-trigger:"#),
                "{artifact} helper occurrence {occurrence} must flatten paginated comment pages via jq slurp before sorting"
            );
        }
    }
}

#[test]
fn pr_bot_artifacts_paginate_reusable_current_head_review_lookup() {
    for artifact in [
        "patterns/pr-bot/workflow.toml",
        "patterns/pr-bot/PATTERN.md",
    ] {
        let text = pr_bot_artifact_text(artifact);
        for occurrence in 0..2 {
            let helper = extract_nth_shell_function(
                &text,
                "query_reusable_current_head_review_record",
                occurrence,
                artifact,
            );
            assert!(
                helper.contains(
                    r#"gh api --paginate "repos/${REPO}/pulls/${PR_NUM}/reviews?per_page=100" 2>/dev/null"#,
                ),
                "{artifact} helper occurrence {occurrence} must paginate pull-request reviews before piping to jq"
            );
            assert!(
                helper.contains(
                    r#"| jq -s '([.[][] | select(.user.login == "'"${CLOUD_BOT_LOGIN}"'")"#
                ),
                "{artifact} helper occurrence {occurrence} must flatten paginated review pages via jq slurp before sorting reusable review records"
            );
            assert!(
                helper.contains(
                    r#"| sort_by(.submitted_at) | last | select(. != null) | [.id, .submitted_at] | @tsv) // ""'"#
                ),
                "{artifact} helper occurrence {occurrence} must guard null reusable review records so empty result variables stay empty instead of rendering a tab"
            );
        }
    }
}

#[test]
fn pr_bot_artifacts_recovery_probe_reuses_null_commit_reviews() {
    for artifact in [
        "patterns/pr-bot/workflow.toml",
        "patterns/pr-bot/PATTERN.md",
    ] {
        let text = pr_bot_artifact_text(artifact);
        for (occurrence, window_var) in [(0, "WAIT_BASE_TS"), (1, "RETRIGGER_TS")] {
            let helper = extract_nth_shell_function(
                &text,
                "query_current_window_current_head_review_ts",
                occurrence,
                artifact,
            );
            assert!(
                helper.contains(
                    r#"gh api --paginate "repos/${REPO}/pulls/${PR_NUM}/reviews?per_page=100" 2>/dev/null"#,
                ),
                "{artifact} recovery helper occurrence {occurrence} must paginate pull-request reviews before piping to jq"
            );
            assert!(
                helper.contains(
                    r#"| jq -s '[.[][] | select(.user.login == "'"${CLOUD_BOT_LOGIN}"'")"#
                ),
                "{artifact} recovery helper occurrence {occurrence} must flatten paginated review pages via jq slurp before selecting current-window signals"
            );
            let expected = format!(
                "select(.submitted_at >= \"'\"${{{}}}\"'\") | select(.commit_id == \"'\"${{CURRENT_SHA}}\"'\" or .commit_id == null) | .submitted_at",
                window_var
            );
            assert!(
                helper.contains(&expected),
                "{artifact} recovery helper occurrence {occurrence} must treat null commit_id reviews as valid current-head signals"
            );
        }
    }
}

#[test]
fn pr_bot_artifacts_paginate_review_event_counts() {
    for artifact in [
        "patterns/pr-bot/workflow.toml",
        "patterns/pr-bot/PATTERN.md",
    ] {
        let text = pr_bot_artifact_text(artifact);
        assert_eq!(
            text.matches(
                r#"gh api --paginate "repos/${REPO}/pulls/${PR_NUM}/reviews?per_page=100""#
            )
            .count(),
            6,
            "{artifact} must keep all six paginated pull-request review queries in pipe-to-jq form"
        );
        assert_eq!(
            text.matches(
                r#"gh api --paginate --slurp "repos/${REPO}/pulls/${PR_NUM}/reviews?per_page=100""#
            )
            .count(),
            0,
            "{artifact} must not use deprecated gh --slurp pagination for pull-request review queries"
        );
        for window_var in ["BOT_REVIEW_WINDOW_START", "POST_FIX_REVIEW_WINDOW_START"] {
            let expected = format!(
                "[.[][] | select(.user.login == \"'\"${{CLOUD_BOT_LOGIN}}\"'\") | select(.submitted_at >= \"'\"${{{}}}\"'\") | select(.commit_id == \"'\"${{CURRENT_SHA}}\"'\" or .commit_id == null)] | length",
                window_var
            );
            assert!(
                text.contains(&expected),
                "{artifact} must flatten paginated review pages before counting review events for {window_var}"
            );
        }
    }
}
