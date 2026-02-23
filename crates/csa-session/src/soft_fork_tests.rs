use super::*;
use crate::output_parser::persist_structured_output;
use crate::result::{RESULT_FILE_NAME, SessionArtifact, SessionResult};
use chrono::Utc;

// ── Context summary from a fully populated parent session ───────────

#[test]
fn test_soft_fork_full_parent_session() {
    let tmp = tempfile::tempdir().unwrap();
    let session_dir = tmp.path();

    // Write result.toml
    let now = Utc::now();
    let result = SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "All tests passed".to_string(),
        tool: "codex".to_string(),
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: vec![SessionArtifact::new("output/diff.patch")],
    };
    let result_toml = toml::to_string_pretty(&result).unwrap();
    std::fs::write(session_dir.join(RESULT_FILE_NAME), &result_toml).unwrap();

    // Write structured output with a summary section
    let output = "<!-- CSA:SECTION:summary -->\n\
                  Implemented feature X with full test coverage.\n\
                  <!-- CSA:SECTION:summary:END -->\n\
                  <!-- CSA:SECTION:details -->\n\
                  Detailed implementation notes here.\n\
                  <!-- CSA:SECTION:details:END -->";
    persist_structured_output(session_dir, output).unwrap();

    let ctx = soft_fork_session(session_dir, "01PARENT123").unwrap();

    assert_eq!(ctx.parent_session_id, "01PARENT123");
    assert!(ctx.context_summary.contains("01PARENT123"));
    assert!(ctx.context_summary.contains("codex"));
    assert!(ctx.context_summary.contains("success"));
    assert!(ctx.context_summary.contains("All tests passed"));
    assert!(ctx.context_summary.contains("summary, details"));
    assert!(ctx.context_summary.contains("Implemented feature X"));
}

// ── Token budget enforcement (truncation) ───────────────────────────

#[test]
fn test_soft_fork_truncation_on_large_summary() {
    let tmp = tempfile::tempdir().unwrap();
    let session_dir = tmp.path();

    // Create result.toml
    let now = Utc::now();
    let result = SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "ok".to_string(),
        tool: "gemini-cli".to_string(),
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: vec![],
    };
    std::fs::write(
        session_dir.join(RESULT_FILE_NAME),
        toml::to_string_pretty(&result).unwrap(),
    )
    .unwrap();

    // Create a very large summary section (>2000 tokens)
    let large_text = "word ".repeat(3000); // ~3000 words = ~4000 tokens
    let output =
        format!("<!-- CSA:SECTION:summary -->\n{large_text}\n<!-- CSA:SECTION:summary:END -->");
    persist_structured_output(session_dir, &output).unwrap();

    let ctx = soft_fork_session(session_dir, "01BIG_PARENT").unwrap();

    // Verify the summary is truncated
    let estimated = estimate_tokens(&ctx.context_summary);
    assert!(
        estimated <= SUMMARY_TOKEN_BUDGET + 50, // small margin for the wrapper text
        "Summary should be within budget, got {} tokens",
        estimated
    );
    assert!(ctx.context_summary.contains("[truncated]"));
}

// ── Empty parent session (no result.toml, no output/) ───────────────

#[test]
fn test_soft_fork_empty_parent_session() {
    let tmp = tempfile::tempdir().unwrap();
    let session_dir = tmp.path();
    // No result.toml, no output/ directory

    let ctx = soft_fork_session(session_dir, "01EMPTY").unwrap();

    assert_eq!(ctx.parent_session_id, "01EMPTY");
    assert!(ctx.context_summary.contains("01EMPTY"));
    assert!(ctx.context_summary.contains("No prior context available"));
}

// ── Parent with result.toml but no structured output ────────────────

#[test]
fn test_soft_fork_result_only_no_output() {
    let tmp = tempfile::tempdir().unwrap();
    let session_dir = tmp.path();

    let now = Utc::now();
    let result = SessionResult {
        status: "failure".to_string(),
        exit_code: 1,
        summary: "Build failed with 3 errors".to_string(),
        tool: "opencode".to_string(),
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: vec![],
    };
    std::fs::write(
        session_dir.join(RESULT_FILE_NAME),
        toml::to_string_pretty(&result).unwrap(),
    )
    .unwrap();

    let ctx = soft_fork_session(session_dir, "01RESULT_ONLY").unwrap();

    assert!(ctx.context_summary.contains("opencode"));
    assert!(ctx.context_summary.contains("failure"));
    assert!(ctx.context_summary.contains("Build failed"));
    // No structured output sections mentioned
    assert!(!ctx.context_summary.contains("Structured output sections"));
}

// ── Parent with structured output but no summary section ────────────

#[test]
fn test_soft_fork_output_without_summary_section() {
    let tmp = tempfile::tempdir().unwrap();
    let session_dir = tmp.path();

    // No result.toml
    // Structured output with only a "details" section, no "summary"
    let output = "<!-- CSA:SECTION:details -->\n\
                  Detailed implementation notes.\n\
                  <!-- CSA:SECTION:details:END -->";
    persist_structured_output(session_dir, output).unwrap();

    let ctx = soft_fork_session(session_dir, "01NO_SUMMARY").unwrap();

    assert!(ctx.context_summary.contains("details"));
    // Should not contain "Summary from parent:" since there's no summary section
    assert!(!ctx.context_summary.contains("Summary from parent:"));
}

// ── truncate_to_token_budget unit tests ─────────────────────────────

#[test]
fn test_truncate_within_budget_returns_unchanged() {
    let text = "short text here";
    let result = truncate_to_token_budget(text, 100);
    assert_eq!(result, text);
}

#[test]
fn test_truncate_over_budget_adds_marker() {
    // 100 words -> ~133 tokens (100 * 4/3)
    let text = (0..100)
        .map(|i| format!("word{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let result = truncate_to_token_budget(&text, 50);
    assert!(result.contains("[truncated]"));
    // Verify the truncated result is within budget
    let estimated = estimate_tokens(&result);
    assert!(
        estimated <= 60,
        "Should be roughly within budget, got {estimated}"
    );
}

#[test]
fn test_truncate_empty_text() {
    let result = truncate_to_token_budget("", 100);
    assert_eq!(result, "");
}

// ── Genealogy integration (soft fork sets fork_of_session_id) ───────

#[test]
fn test_soft_fork_context_provides_parent_id_for_genealogy() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = soft_fork_session(tmp.path(), "01ABCDEF").unwrap();

    // Verify the parent_session_id can be used to set Genealogy.fork_of_session_id
    let genealogy = crate::state::Genealogy {
        fork_of_session_id: Some(ctx.parent_session_id.clone()),
        ..Default::default()
    };
    assert!(genealogy.is_fork());
    assert_eq!(genealogy.fork_source(), Some("01ABCDEF"));
}
