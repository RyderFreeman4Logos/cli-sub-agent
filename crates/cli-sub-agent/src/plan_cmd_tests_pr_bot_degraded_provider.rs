use super::*;
use weave::compiler::ExecutionPlan;

#[tokio::test]
async fn execute_pr_bot_local_review_derives_provider_when_var_unset() {
    let tmp = tempfile::tempdir().unwrap();
    let current_head = "abcdef1234567890abcdef1234567890abcdef12";
    let csa_called_path = install_pr_bot_local_review_stubs(tmp.path(), current_head);
    let wait_args_path = tmp.path().join("session-wait-args");
    let mut vars = pr_bot_local_review_vars(tmp.path(), &csa_called_path);
    vars.insert("CSA_MODEL_PROVIDER".into(), String::new());
    vars.insert("CSA_CALLER_TOOL".into(), "hermes".into());
    vars.insert("HERMES_MODEL_PROVIDER".into(), "xai-oauth".into());
    vars.insert("HOME".into(), tmp.path().display().to_string());
    vars.insert("TEST_CSA_REVIEW_MODE".into(), "success".into());
    vars.insert(
        "TEST_CSA_SESSION_WAIT_ARGS".into(),
        wait_args_path.display().to_string(),
    );
    let (variables, steps) =
        pr_bot_plan_steps_by_title(&["Local Pre-PR Review (SYNCHRONOUS — MUST NOT background)"]);
    let plan = ExecutionPlan {
        name: "pr-bot-derived-provider-wait".into(),
        description: String::new(),
        variables,
        steps,
    };

    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .expect("derived-provider review wait should execute");

    assert_eq!(results[0].exit_code, 0, "local review should pass");
    let wait_args = std::fs::read_to_string(&wait_args_path).unwrap();
    assert!(
        wait_args.contains("--model-provider xai"),
        "session wait must derive the caller provider: {wait_args}"
    );
}

#[tokio::test]
async fn execute_pr_bot_bot_unavailable_wait_derives_provider() {
    let tmp = tempfile::tempdir().unwrap();
    let current_head = "abcdef1234567890abcdef1234567890abcdef12";
    let csa_called_path = install_pr_bot_local_review_stubs(tmp.path(), current_head);
    let wait_args_path = tmp.path().join("session-wait-args");
    let mut vars = pr_bot_local_review_vars(tmp.path(), &csa_called_path);
    vars.insert("CSA_MODEL_PROVIDER".into(), String::new());
    vars.insert("CSA_CALLER_TOOL".into(), "hermes".into());
    vars.insert("HERMES_MODEL_PROVIDER".into(), "zhipuai".into());
    vars.insert("HOME".into(), tmp.path().display().to_string());
    vars.insert("MERGE_COMPLETED".into(), "false".into());
    vars.insert("TEST_CLOUD_BOT".into(), "false".into());
    vars.insert("PR_NUM".into(), "1788".into());
    vars.insert("REPO".into(), "RyderFreeman4Logos/cli-sub-agent".into());
    vars.insert("TEST_CSA_REVIEW_MODE".into(), "success".into());
    vars.insert(
        "TEST_CSA_SESSION_WAIT_ARGS".into(),
        wait_args_path.display().to_string(),
    );
    let (variables, steps) =
        pr_bot_plan_steps_by_title(&["Step 4a: Check Cloud Bot Configuration"]);
    let plan = ExecutionPlan {
        name: "pr-bot-step4a-derived-provider-wait".into(),
        description: String::new(),
        variables,
        steps,
    };

    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .expect("bot-unavailable derived-provider wait should execute");

    assert_eq!(results[0].exit_code, 0, "Step 4a should pass");
    let wait_args = std::fs::read_to_string(&wait_args_path).unwrap();
    assert!(
        wait_args.contains("--model-provider glm"),
        "Step 4a wait must derive the caller provider: {wait_args}"
    );
}

#[tokio::test]
async fn execute_pr_bot_local_review_preserves_explicit_provider_key() {
    let tmp = tempfile::tempdir().unwrap();
    let current_head = "abcdef1234567890abcdef1234567890abcdef12";
    let csa_called_path = install_pr_bot_local_review_stubs(tmp.path(), current_head);
    let wait_args_path = tmp.path().join("session-wait-args");
    let mut vars = pr_bot_local_review_vars(tmp.path(), &csa_called_path);
    vars.insert("CSA_MODEL_PROVIDER".into(), "  AnThRoPiC  ".into());
    vars.insert("HOME".into(), tmp.path().display().to_string());
    vars.insert("TEST_CSA_REVIEW_MODE".into(), "success".into());
    vars.insert(
        "TEST_CSA_SESSION_WAIT_ARGS".into(),
        wait_args_path.display().to_string(),
    );
    let (variables, steps) =
        pr_bot_plan_steps_by_title(&["Local Pre-PR Review (SYNCHRONOUS — MUST NOT background)"]);
    let plan = ExecutionPlan {
        name: "pr-bot-explicit-provider-key".into(),
        description: String::new(),
        variables,
        steps,
    };

    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .expect("explicit-provider review wait should execute");

    assert_eq!(results[0].exit_code, 0, "local review should pass");
    let wait_args = std::fs::read_to_string(&wait_args_path).unwrap();
    assert!(
        wait_args.contains("--model-provider anthropic"),
        "session wait must preserve an explicit configured key: {wait_args}"
    );
}

#[tokio::test]
async fn execute_pr_bot_post_fix_nonzero_wait_mode_still_accepts_exact_head_native_bypass() {
    let tmp = tempfile::tempdir().unwrap();
    let current_head = "abcdef1234567890abcdef1234567890abcdef12";
    let csa_called_path = install_pr_bot_local_review_stubs(tmp.path(), current_head);
    write_native_review_bypass_artifact(tmp.path(), current_head);
    let wait_args_path = tmp.path().join("session-wait-args");

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
    vars.insert("TEST_SESSION_WAIT_NONZERO".into(), "true".into());
    vars.insert(
        "TEST_CSA_SESSION_WAIT_ARGS".into(),
        wait_args_path.display().to_string(),
    );
    let (variables, steps) =
        pr_bot_plan_steps_by_title(&["Step 10b: Post-Fix Re-Review Gate (HARD GATE)"]);
    let plan = ExecutionPlan {
        name: "pr-bot-post-fix-nonzero-wait-native-bypass".into(),
        description: String::new(),
        variables,
        steps,
    };

    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .expect("nonzero wait-mode must still accept exact-head native bypass");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].exit_code, 0, "Step 10b should pass");
    let output = results[0].output.as_deref().unwrap_or("");
    assert!(
        output.contains("Local fallback native review bypass covers HEAD"),
        "nonzero wait-mode must still take the exact-head native-bypass path: {output}"
    );
    assert!(
        output.contains("CSA_VAR:LOCAL_REVIEW_SESSION_ID=native-review-bypass-abcdef123456"),
        "native fallback should publish a bounded synthetic session id: {output}"
    );
    assert!(
        !csa_called_path.exists(),
        "exact-head native bypass must still avoid a CSA review launch after nonzero wait"
    );
    let wait_args = std::fs::read_to_string(&wait_args_path).unwrap();
    assert!(
        wait_args.contains("--model-provider openai"),
        "nonzero wait-mode must still forward the provider: {wait_args}"
    );
}
