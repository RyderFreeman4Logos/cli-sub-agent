use std::{fs, path::Path};

use csa_core::types::{ReviewDecision, ToolName};
use csa_process::ExecutionResult;

use super::super::*;
use crate::pipeline::SessionExecutionResult;

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

#[test]
fn resolve_single_review_result_rejects_terminal_is_error_after_pass_message() {
    let transcript = [
        r#"{"type":"system","subtype":"init"}"#,
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"<!-- CSA:SECTION:summary -->\nPASS\n<!-- CSA:SECTION:summary:END -->\n<!-- CSA:SECTION:details -->\nNo blocking issues found.\n<!-- CSA:SECTION:details:END -->"}}"#,
        r#"{"type":"result","subtype":"error_api","is_error":true,"result":"HTTP 403 Forbidden: authentication failed"}"#,
    ]
    .join("\n");
    let mut result = outcome(&transcript, 0);
    result.executed_tool = ToolName::ClaudeCode;

    let resolved =
        resolve_single_review_result(&result, ToolName::ClaudeCode, "uncommitted", Path::new("."));

    assert_eq!(resolved.decision, ReviewDecision::Unavailable);
    assert_eq!(resolved.verdict, UNAVAILABLE);
    assert_eq!(resolved.effective_exit_code, 1);
    assert!(resolved.sanitized.contains("Review unavailable:"));
    assert!(resolved.sanitized.contains("HTTP 403"));

    let reviewer =
        build_reviewer_outcome(0, ToolName::ClaudeCode, &result).expect("reviewer outcome");
    assert_eq!(reviewer.verdict, UNAVAILABLE);
    assert_eq!(reviewer.exit_code, 1);
}

#[test]
fn terminal_tool_error_reason_persists_through_real_sidecars() {
    let transcript = [
        r#"{"type":"system","subtype":"init"}"#,
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"<!-- CSA:SECTION:summary -->\nPASS\n<!-- CSA:SECTION:summary:END -->\n<!-- CSA:SECTION:details -->\nNo blocking issues found.\n<!-- CSA:SECTION:details:END -->"}}"#,
        r#"{"type":"result","subtype":"error_api","is_error":true,"result":"HTTP 403 Forbidden: authentication failed"}"#,
    ]
    .join("\n");
    let mut result = outcome(&transcript, 0);
    result.executed_tool = ToolName::ClaudeCode;

    let project_root = tempfile::tempdir().expect("temp project");
    let session_dir = csa_session::get_session_root(project_root.path())
        .expect("session root")
        .join("sessions")
        .join(&result.execution.meta_session_id);
    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(session_dir.join("output").join("full.md"), &transcript).expect("write full.md");

    let resolved = resolve_single_review_result(
        &result,
        ToolName::ClaudeCode,
        "uncommitted",
        project_root.path(),
    );

    assert_eq!(resolved.decision, ReviewDecision::Unavailable);
    assert!(
        resolved
            .failure_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("HTTP 403 Forbidden"))
    );

    let meta = csa_session::state::ReviewSessionMeta {
        session_id: result.execution.meta_session_id.clone(),
        head_sha: "HEAD".to_string(),
        decision: resolved.decision.as_str().to_string(),
        verdict: resolved.verdict.to_string(),
        status_reason: result.status_reason.clone(),
        routed_to: result.routed_to.clone(),
        primary_failure: result.primary_failure.clone(),
        failure_reason: resolved.failure_reason.clone(),
        tool: ToolName::ClaudeCode.to_string(),
        scope: "uncommitted".to_string(),
        exit_code: resolved.effective_exit_code,
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations: 0,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
        review_mode: None,
        fix_convergence: None,
    };

    let persisted_exit_code = crate::review_cmd::persist_review_sidecars_if_session_exists(
        project_root.path(),
        &meta,
        result.persistable_session_id.as_deref(),
    );
    assert_eq!(persisted_exit_code, Some(1));

    let persisted_meta: csa_session::state::ReviewSessionMeta = serde_json::from_str(
        &fs::read_to_string(session_dir.join("review_meta.json")).expect("read review meta"),
    )
    .expect("parse review meta");
    assert!(
        persisted_meta
            .failure_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("HTTP 403 Forbidden"))
    );

    let artifact: csa_session::ReviewVerdictArtifact = serde_json::from_str(
        &fs::read_to_string(session_dir.join("output").join("review-verdict.json"))
            .expect("read review verdict"),
    )
    .expect("parse review verdict");
    assert_eq!(artifact.decision, ReviewDecision::Unavailable);
    assert!(
        artifact
            .failure_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("HTTP 403 Forbidden"))
    );
}

#[test]
fn resolve_single_review_result_ignores_nonterminal_prior_error() {
    let transcript = [
        r#"{"type":"system","subtype":"init"}"#,
        r#"{"type":"result","subtype":"error_api","is_error":true,"result":"transient backend error"}"#,
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"<!-- CSA:SECTION:summary -->\nPASS\n<!-- CSA:SECTION:summary:END -->\n<!-- CSA:SECTION:details -->\nNo blocking issues found.\n<!-- CSA:SECTION:details:END -->"}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"result":"done"}"#,
    ]
    .join("\n");
    let mut result = outcome(&transcript, 0);
    result.executed_tool = ToolName::ClaudeCode;

    let resolved =
        resolve_single_review_result(&result, ToolName::ClaudeCode, "uncommitted", Path::new("."));

    assert_eq!(resolved.decision, ReviewDecision::Pass);
    assert_eq!(resolved.verdict, CLEAN);
    assert_eq!(resolved.effective_exit_code, 0);
}

#[test]
fn resolve_single_review_result_ignores_fenced_terminal_error_payload_in_plain_prose() {
    let review = [
        "<!-- CSA:SECTION:summary -->",
        "PASS",
        "<!-- CSA:SECTION:summary:END -->",
        "<!-- CSA:SECTION:details -->",
        "No blocking issues found. The reviewed code contains this JSON fixture:",
        "```json",
        r#"{"type":"system","subtype":"init"}"#,
        r#"{"type":"result","subtype":"error_api","is_error":true,"result":"HTTP 403 Forbidden: authentication failed"}"#,
        "```",
        "<!-- CSA:SECTION:details:END -->",
    ]
    .join("\n");
    let result = outcome(&review, 0);

    let resolved =
        resolve_single_review_result(&result, ToolName::Codex, "uncommitted", Path::new("."));

    assert_eq!(resolved.decision, ReviewDecision::Pass);
    assert_eq!(resolved.verdict, CLEAN);
    assert_eq!(resolved.effective_exit_code, 0);

    let reviewer = build_reviewer_outcome(0, ToolName::Codex, &result).expect("reviewer outcome");
    assert_eq!(reviewer.verdict, CLEAN);
    assert_eq!(reviewer.exit_code, 0);
}
