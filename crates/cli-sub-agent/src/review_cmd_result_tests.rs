use super::*;
use crate::pipeline::SessionExecutionResult;
use csa_core::env::{CSA_SESSION_DIR_ENV_KEY, CSA_SESSION_ID_ENV_KEY};
use csa_process::ExecutionResult;
use std::{collections::HashMap, fs, path::Path};

fn outcome(output: &str, exit_code: i32) -> ReviewExecutionOutcome {
    ReviewExecutionOutcome {
        execution: SessionExecutionResult {
            execution: ExecutionResult {
                output: output.to_string(),
                stderr_output: String::new(),
                summary: String::new(),
                exit_code,
                peak_memory_mb: None,
                ..Default::default()
            },
            meta_session_id: "01TESTRESULT".to_string(),
            provider_session_id: None,
            changed_paths: None,
        },
        persistable_session_id: Some("01TESTRESULT".to_string()),
        executed_tool: ToolName::Codex,
        status_reason: None,
        forced_decision: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
    }
}

fn startup_env_for_parent_session(
    session_dir: &Path,
    session_id: &str,
) -> crate::startup_env::StartupSubtreeEnv {
    crate::startup_env::StartupSubtreeEnv::from_values(HashMap::from([
        (CSA_SESSION_DIR_ENV_KEY, session_dir.display().to_string()),
        (CSA_SESSION_ID_ENV_KEY, session_id.to_string()),
    ]))
}

#[test]
fn resolve_single_review_result_preserves_uncertain_contract() {
    let resolved = resolve_single_review_result(
        &outcome(
            "<!-- CSA:SECTION:summary -->\nUNCERTAIN\n<!-- CSA:SECTION:summary:END -->",
            0,
        ),
        ToolName::Codex,
        "uncommitted",
        Path::new("."),
    );

    assert_eq!(resolved.decision, ReviewDecision::Uncertain);
    assert_eq!(resolved.verdict, UNCERTAIN);
    assert_eq!(resolved.effective_exit_code, 1);
}

#[test]
fn resolve_single_review_result_maps_transient_gemini_failure_to_unavailable() {
    let mut result = outcome("", 1);
    result.executed_tool = ToolName::GeminiCli;
    result.execution.execution.summary = "retryDelayMs: undefined".to_string();

    let resolved =
        resolve_single_review_result(&result, ToolName::GeminiCli, "uncommitted", Path::new("."));

    assert_eq!(resolved.decision, ReviewDecision::Unavailable);
    assert_eq!(resolved.verdict, UNAVAILABLE);
    assert_eq!(resolved.effective_exit_code, 1);
    assert!(resolved.sanitized.contains("Review unavailable:"));
    assert!(resolved.sanitized.contains("retryDelayMs: undefined"));
}

#[test]
fn build_reviewer_outcome_maps_quota_limited_tool_to_unavailable() {
    let mut result = outcome(
        r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"seven_day"}}"#,
        1,
    );
    result.execution.execution.summary =
        r#"{"api_error_status":429,"result":"You've hit your org's monthly usage limit"}"#
            .to_string();

    let reviewer = build_reviewer_outcome(0, ToolName::Codex, &result).expect("reviewer outcome");

    assert_eq!(reviewer.verdict, UNAVAILABLE);
    assert_eq!(reviewer.exit_code, 1);
    assert!(reviewer.output.contains("Review unavailable:"));
    assert!(
        reviewer
            .diagnostic
            .as_deref()
            .is_some_and(|diagnostic| diagnostic.contains("monthly usage limit"))
    );
}

#[test]
fn build_reviewer_outcome_maps_api_key_failure_to_unavailable() {
    let mut result = outcome("", 1);
    result.executed_tool = ToolName::GeminiCli;
    result.execution.execution.stderr_output = r#"API Key not found.
details: [{"reason":"API_KEY_INVALID","domain":"googleapis.com"}]"#
        .to_string();

    let reviewer =
        build_reviewer_outcome(0, ToolName::GeminiCli, &result).expect("reviewer outcome");

    assert_eq!(reviewer.verdict, UNAVAILABLE);
    assert_eq!(reviewer.exit_code, 1);
    assert!(reviewer.output.contains("Review unavailable:"));
    assert!(
        reviewer
            .diagnostic
            .as_deref()
            .is_some_and(|diagnostic| diagnostic.contains("API_KEY_INVALID"))
    );
}

#[test]
fn reviewer_unavailable_error_reason_maps_quota_exhausted_error() {
    let err = anyhow::anyhow!(
        "gemini-cli failed: status: RESOURCE_EXHAUSTED; reason: QUOTA_EXHAUSTED; HTTP 429"
    );

    let reason =
        reviewer_unavailable_error_reason(&err, ToolName::GeminiCli).expect("quota reason");

    assert!(reason.contains("gemini-cli tool failure"));
    assert!(reason.contains("QUOTA_EXHAUSTED"));
}

#[test]
fn reviewer_unavailable_error_reason_maps_api_key_invalid_error() {
    let err = anyhow::anyhow!("gemini-cli failed: status 400 API_KEY_INVALID");

    let reason = reviewer_unavailable_error_reason(&err, ToolName::GeminiCli).expect("auth reason");

    assert!(reason.contains("gemini-cli tool failure"));
    assert!(reason.contains("API_KEY_INVALID"));
}

#[test]
fn reviewer_unavailable_error_reason_maps_backend_400_error() {
    let err = anyhow::anyhow!(
        "claude-code failed: status: 400 Bad Request: thinking blocks must be preserved"
    );

    let reason = reviewer_unavailable_error_reason(&err, ToolName::ClaudeCode).expect("400 reason");

    assert!(reason.contains("claude-code tool failure"));
    assert!(reason.contains("400 Bad Request"));
}

#[test]
fn resolve_single_review_result_maps_api_key_failure_to_unavailable() {
    let mut result = outcome("", 1);
    result.executed_tool = ToolName::GeminiCli;
    result.execution.execution.stderr_output =
        "status: 400 API_KEY_INVALID API Key not found".to_string();

    let resolved =
        resolve_single_review_result(&result, ToolName::GeminiCli, "uncommitted", Path::new("."));

    assert_eq!(resolved.decision, ReviewDecision::Unavailable);
    assert_eq!(resolved.verdict, UNAVAILABLE);
    assert_eq!(resolved.effective_exit_code, 1);
    assert!(resolved.sanitized.contains("Review unavailable:"));
}

#[test]
fn build_reviewer_outcome_does_not_mark_review_prose_quota_mentions_unavailable() {
    let reviewer = build_reviewer_outcome(
        0,
        ToolName::Codex,
        &outcome(
            "Substantive review finding: quota handling drops errors but no explicit verdict.",
            1,
        ),
    )
    .expect("reviewer outcome");

    assert_ne!(reviewer.verdict, UNAVAILABLE);
    assert!(!reviewer.output.contains("Review unavailable:"));
    assert!(reviewer.diagnostic.is_none());
}

#[test]
fn genuine_provider_error_maps_to_unavailable() {
    let mut result = outcome("", 1);
    result.execution.execution.stderr_output =
        "provider request failed: HTTP 503 Service Unavailable".to_string();

    let resolved = resolve_single_review_result(&result, ToolName::Codex, "diff", Path::new("."));

    assert_eq!(resolved.decision, ReviewDecision::Unavailable);
    assert_eq!(resolved.verdict, UNAVAILABLE);
    assert_eq!(resolved.effective_exit_code, 1);
    assert!(resolved.sanitized.contains("Review unavailable:"));
}

#[test]
fn tool_unavailable_failure_does_not_override_explicit_fail_verdict() {
    let reviewer = build_reviewer_outcome(
        0,
        ToolName::Codex,
        &outcome(
            "<!-- CSA:SECTION:summary -->\nFAIL\n<!-- CSA:SECTION:summary:END -->\n\
             <!-- CSA:SECTION:details -->\nA real finding mentions rate limit handling.\n<!-- CSA:SECTION:details:END -->",
            1,
        ),
    )
    .expect("reviewer outcome");

    assert_eq!(reviewer.verdict, HAS_ISSUES);
    assert!(!reviewer.output.contains("Review unavailable:"));
}

#[test]
fn resolve_single_review_result_uses_last_reviewer_message_not_prompt_tokens() {
    let raw = concat!(
        "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"Reviewer instructions mention HAS_ISSUES as a legacy alias.\"}}\n",
        "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"<!-- CSA:SECTION:summary -->\\nPASS\\n<!-- CSA:SECTION:summary:END -->\\n\\n<!-- CSA:SECTION:details -->\\nNo blocking correctness issues found.\\n<!-- CSA:SECTION:details:END -->\"}}\n",
    );

    let resolved = resolve_single_review_result(
        &outcome(raw, 0),
        ToolName::Codex,
        "uncommitted",
        Path::new("."),
    );

    assert_eq!(resolved.decision, ReviewDecision::Pass);
    assert_eq!(resolved.verdict, CLEAN);
    assert_eq!(resolved.effective_exit_code, 0);
}

#[test]
fn resolve_single_review_result_falls_back_to_summary_md_for_clear_verdict() {
    let project_root = tempfile::tempdir().expect("temp project");
    let session_dir = csa_session::get_session_root(project_root.path())
        .expect("session root")
        .join("sessions")
        .join("01TESTRESULT");
    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(session_dir.join("output").join("summary.md"), "**PASS**\n").expect("write summary");

    let resolved = resolve_single_review_result(
        &outcome("review completed without a verdict token", 1),
        ToolName::Codex,
        "uncommitted",
        project_root.path(),
    );

    assert_eq!(resolved.decision, ReviewDecision::Pass);
    assert_eq!(resolved.verdict, CLEAN);
}

#[test]
fn mock_pass_review_persists_pass_review_meta() {
    let project_root = tempfile::tempdir().expect("temp project");
    let session_dir = csa_session::get_session_root(project_root.path())
        .expect("session root")
        .join("sessions")
        .join("01TESTRESULT");
    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    let raw = concat!(
        "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"Prompt context: legacy HAS_ISSUES maps to failure.\"}}\n",
        "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"<!-- CSA:SECTION:summary -->\\nPASS\\n<!-- CSA:SECTION:summary:END -->\\n\\n<!-- CSA:SECTION:details -->\\nNo blocking correctness issues found.\\n<!-- CSA:SECTION:details:END -->\"}}\n",
    );
    let resolved = resolve_single_review_result(
        &outcome(raw, 0),
        ToolName::Codex,
        "uncommitted",
        project_root.path(),
    );
    let meta = csa_session::state::ReviewSessionMeta {
        session_id: "01TESTRESULT".to_string(),
        head_sha: "HEAD".to_string(),
        decision: resolved.decision.as_str().to_string(),
        verdict: resolved.verdict.to_string(),
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
        tool: ToolName::Codex.to_string(),
        scope: "uncommitted".to_string(),
        exit_code: resolved.effective_exit_code,
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations: 0,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
        fix_convergence: None,
    };

    csa_session::state::write_review_meta(&session_dir, &meta).expect("write review meta");
    let persisted: csa_session::state::ReviewSessionMeta = serde_json::from_str(
        &fs::read_to_string(session_dir.join("review_meta.json")).expect("read review meta"),
    )
    .expect("parse review meta");

    assert_eq!(persisted.decision, ReviewDecision::Pass.as_str());
    assert_eq!(persisted.verdict, CLEAN);
}

#[test]
fn build_reviewer_outcome_preserves_skip_contract() {
    let reviewer = build_reviewer_outcome(
        0,
        ToolName::Codex,
        &outcome(
            "<!-- CSA:SECTION:summary -->\nSKIP\n<!-- CSA:SECTION:summary:END -->",
            0,
        ),
    )
    .expect("reviewer outcome");

    assert_eq!(reviewer.verdict, SKIP);
    assert_eq!(reviewer.exit_code, 1);
}

#[test]
fn forced_unavailable_preserves_explicit_pass_verdict() {
    let mut result = outcome(
        "<!-- CSA:SECTION:summary -->\nPASS\n<!-- CSA:SECTION:summary:END -->\n\
         <!-- CSA:SECTION:details -->\nfindings = []\n<!-- CSA:SECTION:details:END -->",
        1,
    );
    result.forced_decision = Some(ReviewDecision::Unavailable);
    result.failure_reason =
        Some("claude-code post-review state write failed: EROFS ~/.claude.json".to_string());

    let resolved =
        resolve_single_review_result(&result, ToolName::ClaudeCode, "uncommitted", Path::new("."));
    assert_eq!(resolved.decision, ReviewDecision::Pass);
    assert_eq!(resolved.verdict, CLEAN);
    assert_eq!(resolved.effective_exit_code, 0);

    let reviewer =
        build_reviewer_outcome(0, ToolName::ClaudeCode, &result).expect("reviewer outcome");
    assert_eq!(reviewer.verdict, CLEAN);
    assert_eq!(reviewer.exit_code, 0);
}

/// A claude-code reviewer killed mid-turn: the stream begins (system/assistant/agent_message
/// events) but never emits the terminal `{"type":"result"}`. Its last agent message narrates the
/// reviewed code, which itself contains a `HAS_ISSUES` literal — the exact verdict-token
/// contamination that made the round-4 runtime review fail-close to HAS_ISSUES instead of
/// UNAVAILABLE (#1657).
fn killed_claude_code_review_transcript() -> String {
    [
        r#"{"type":"system","subtype":"init","session_id":"01TESTKILLEDCLAUDE"}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":"reviewing"}}"#,
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"parse_review_decision returns HAS_ISSUES when a blocking token appears in the reviewed diff."}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta"}}"#,
    ]
    .join("\n")
}

#[test]
fn build_reviewer_outcome_reclassifies_killed_claude_code_stream_as_unavailable() {
    // Pre-fix: extract_review_text scrapes the partial agent message's HAS_ISSUES token and the
    // reviewer fail-closes to HAS_ISSUES. Post-fix: the stream has no terminal `result` event and
    // exited non-zero, so it is classified UNAVAILABLE (infra could not run to completion).
    let mut result = outcome(&killed_claude_code_review_transcript(), 1);
    result.executed_tool = ToolName::ClaudeCode;

    let reviewer =
        build_reviewer_outcome(0, ToolName::ClaudeCode, &result).expect("reviewer outcome");

    assert_eq!(reviewer.verdict, UNAVAILABLE);
    assert_eq!(reviewer.exit_code, 1);
}

#[test]
fn build_reviewer_outcome_preserves_completed_has_issues_verdict() {
    // A reviewer that ran to completion (terminal `result` event present) keeps its deliberate
    // HAS_ISSUES verdict even with a non-zero exit — the killed-stream override must not fire.
    let mut transcript = killed_claude_code_review_transcript();
    transcript.push('\n');
    transcript
        .push_str(r#"{"type":"result","subtype":"success","result":"HAS_ISSUES: blocking bug at src/foo.rs:10"}"#);
    let mut result = outcome(&transcript, 1);
    result.executed_tool = ToolName::ClaudeCode;

    let reviewer =
        build_reviewer_outcome(0, ToolName::ClaudeCode, &result).expect("reviewer outcome");

    assert_eq!(reviewer.verdict, HAS_ISSUES);
    assert_eq!(reviewer.exit_code, 1);
}

#[test]
fn build_reviewer_outcome_preserves_completed_clean_verdict() {
    let transcript = [
        r#"{"type":"system","subtype":"init"}"#,
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"CLEAN: no blocking issues found in the diff."}}"#,
        r#"{"type":"result","subtype":"success","result":"CLEAN: no blocking issues found in the diff."}"#,
    ]
    .join("\n");
    let mut result = outcome(&transcript, 0);
    result.executed_tool = ToolName::ClaudeCode;

    let reviewer =
        build_reviewer_outcome(0, ToolName::ClaudeCode, &result).expect("reviewer outcome");

    assert_eq!(reviewer.verdict, CLEAN);
    assert_eq!(reviewer.exit_code, 0);
}

#[test]
fn build_reviewer_outcome_reclassifies_killed_codex_stream_as_unavailable() {
    // codex equivalent: a stream with `turn.failed` (killed by signal) but no `turn.completed`.
    let transcript = [
        r#"{"type":"thread.started","thread_id":"t-kill"}"#,
        r#"{"type":"turn.started"}"#,
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"The verdict mapping emits HAS_ISSUES for blocking findings in the reviewed code."}}"#,
        r#"{"type":"turn.failed","error":{"message":"stream disconnected before completion: killed by signal 9"}}"#,
    ]
    .join("\n");
    let result = outcome(&transcript, 1);

    let reviewer = build_reviewer_outcome(0, ToolName::Codex, &result).expect("reviewer outcome");

    assert_eq!(reviewer.verdict, UNAVAILABLE);
    assert_eq!(reviewer.exit_code, 1);
}

#[test]
fn build_reviewer_outcome_keeps_plain_text_blocking_verdict_without_stream() {
    // Legacy plain-text reviewer output (gemini-cli / opencode) is not a JSON stream, so the
    // killed-stream override never fires: a genuine blocking verdict is preserved, not masked.
    let reviewer = build_reviewer_outcome(
        0,
        ToolName::GeminiCli,
        &outcome(
            "Overall risk: high\nHAS_ISSUES: a real blocking correctness bug in the patch.",
            1,
        ),
    )
    .expect("reviewer outcome");

    assert_eq!(reviewer.verdict, HAS_ISSUES);
}

#[test]
fn stream_started_without_terminal_event_flags_killed_claude_code_stream() {
    assert!(stream_started_without_terminal_event(
        &killed_claude_code_review_transcript()
    ));
}

#[test]
fn stream_started_without_terminal_event_clears_on_claude_code_result_event() {
    let mut transcript = killed_claude_code_review_transcript();
    transcript.push('\n');
    transcript.push_str(r#"{"type":"result","subtype":"success","result":"CLEAN"}"#);
    assert!(!stream_started_without_terminal_event(&transcript));
}

#[test]
fn stream_started_without_terminal_event_clears_on_codex_turn_completed() {
    let transcript = [
        r#"{"type":"turn.started"}"#,
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"done"}}"#,
        r#"{"type":"turn.completed","usage":{"input_tokens":1}}"#,
    ]
    .join("\n");
    assert!(!stream_started_without_terminal_event(&transcript));
}

#[test]
fn stream_started_without_terminal_event_ignores_non_stream_output() {
    // Plain-text reviewer output and rate-limit event blobs are not recognized streams.
    assert!(!stream_started_without_terminal_event(
        "Overall risk: high\nHAS_ISSUES"
    ));
    assert!(!stream_started_without_terminal_event(
        r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected"}}"#
    ));
    assert!(!stream_started_without_terminal_event(""));
}

#[test]
fn all_killed_reviewers_persist_unavailable_decision_on_disk() {
    // End-to-end reproduction of the round-4 runtime bug: three reviewers all killed mid-turn,
    // run through the REAL derivation (`build_reviewer_outcome`) and the parent write path, then
    // read back from disk. Pre-fix each killed reviewer mis-derived to HAS_ISSUES, so
    // `all_reviewers_unavailable` was false and the parent persisted `decision: "fail"`.
    // Post-fix each derives UNAVAILABLE → the parent persists the distinct `decision: "unavailable"`.
    let _env_lock = crate::test_env_lock::TEST_ENV_LOCK.blocking_lock();
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let session_dir = temp.path().display().to_string();
    let _session_dir_guard = crate::test_env_lock::ScopedEnvVarRestore::set(
        csa_core::env::CSA_SESSION_DIR_ENV_KEY,
        &session_dir,
    );
    let _session_id_guard = crate::test_env_lock::ScopedEnvVarRestore::set(
        "CSA_SESSION_ID",
        "01PARENTSESSION000000000000",
    );
    let _daemon_session_dir_guard =
        crate::test_env_lock::ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_DIR");
    let _daemon_session_id_guard =
        crate::test_env_lock::ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_ID");

    let killed = killed_claude_code_review_transcript();
    let outcomes: Vec<_> = (0..3)
        .map(|index| {
            let mut result = outcome(&killed, 1);
            result.executed_tool = ToolName::ClaudeCode;
            build_reviewer_outcome(index, ToolName::ClaudeCode, &result).expect("reviewer outcome")
        })
        .collect();

    assert!(
        outcomes
            .iter()
            .all(|reviewer| reviewer.verdict == UNAVAILABLE),
        "every reviewer killed mid-turn must derive UNAVAILABLE (pre-fix they fail-closed to \
         HAS_ISSUES); got {:?}",
        outcomes
            .iter()
            .map(|reviewer| reviewer.verdict)
            .collect::<Vec<_>>()
    );

    // Mirror the runtime aggregation in review_cmd_multi.rs: `all_reviewers_unavailable` and the
    // `final_verdict` are COMPUTED from the derived outcomes, not hardcoded — pre-fix this yields
    // (false, HAS_ISSUES) and the disk decision below would be `fail`.
    let all_reviewers_unavailable = !outcomes.is_empty()
        && outcomes
            .iter()
            .all(|reviewer| reviewer.verdict == UNAVAILABLE);
    let final_verdict = if all_reviewers_unavailable {
        UNAVAILABLE
    } else {
        HAS_ISSUES
    };

    super::super::parent_artifacts::write_multi_reviewer_parent_artifacts(
        temp.path(),
        outcomes.len(),
        &outcomes,
        final_verdict,
        all_reviewers_unavailable,
        &startup_env_for_parent_session(temp.path(), "01PARENTSESSION000000000000"),
        None,
    )
    .expect("parent artifacts should be produced");

    let verdict: csa_session::review_artifact::ReviewVerdictArtifact = serde_json::from_str(
        &fs::read_to_string(temp.path().join("output").join("review-verdict.json"))
            .expect("review-verdict.json should exist"),
    )
    .expect("review verdict should parse");

    // Distinguishable at rest + fail-closed: the persisted decision is the distinct `unavailable`
    // value, never `fail`/`uncertain`, and remains non-mergeable.
    assert_eq!(verdict.decision, ReviewDecision::Unavailable);
    assert_ne!(verdict.decision, ReviewDecision::Fail);
    assert_ne!(verdict.decision, ReviewDecision::Uncertain);
    assert!(
        !verdict.decision.is_clean(),
        "unavailable must remain non-mergeable (fail-closed)"
    );
    assert_eq!(
        crate::verdict_exit_code::exit_code_from_review_decision(verdict.decision),
        1
    );
}
