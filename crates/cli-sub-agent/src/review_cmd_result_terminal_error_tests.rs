use std::path::Path;

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
