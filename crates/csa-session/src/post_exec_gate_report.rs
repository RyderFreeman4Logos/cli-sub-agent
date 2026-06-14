//! Structured post-exec-gate failure detail persisted into `result.toml` (#1726).
//!
//! When a `csa run` write session's post-exec verification gate (e.g.
//! `just pre-commit`) fails, the gate's stdout/stderr used to land ONLY in the
//! raw transcript `output/full.md`. An SA-Layer-0 orchestrator (which is
//! forbidden from reading transcripts) therefore could not diagnose the failing
//! step/test from any structured artifact.
//!
//! This module owns the typed [`PostExecGateReport`] (the `[post_exec_gate]`
//! table of `result.toml`) plus the pure parsers/bounding helpers used to build
//! it. The unbounded gate output is written separately to
//! [`GATE_FAILURE_LOG_REL_PATH`]; only a bounded [`PostExecGateReport::output_tail`]
//! is embedded in `result.toml` to keep that envelope small.

use serde::{Deserialize, Serialize};

/// Relative path (from the session directory) of the full, unbounded gate
/// output log written on a post-exec gate failure.
pub const GATE_FAILURE_LOG_REL_PATH: &str = "output/gate-failure.log";

/// Hard cap on the number of trailing lines retained in
/// [`PostExecGateReport::output_tail`].
pub const GATE_OUTPUT_TAIL_MAX_LINES: usize = 100;

/// Hard cap on the byte length retained in [`PostExecGateReport::output_tail`].
/// The full output always lives in [`GATE_FAILURE_LOG_REL_PATH`].
pub const GATE_OUTPUT_TAIL_MAX_BYTES: usize = 8 * 1024;

/// Leading marker for gate-failure summaries. Callers and tests key off this
/// marker to distinguish the authoritative gate verdict from the employee's
/// pre-gate self-report.
pub const GATE_SUMMARY_LEAD: &str = "POST-EXEC GATE FAILED";

/// How many failing test names to inline into one-line summaries before
/// collapsing the rest into a `(+N more)` suffix.
const SUMMARY_MAX_INLINE_TESTS: usize = 5;

/// Maximum size of the single-line tail excerpt embedded in summary text.
const SUMMARY_OUTPUT_EXCERPT_MAX_CHARS: usize = 240;

/// Structured detail of a failed post-exec verification gate.
///
/// Serialized as the `[post_exec_gate]` table of `result.toml`. Present ONLY
/// when the gate failed; the field on [`crate::result::SessionResult`] is
/// `Option`-wrapped with `skip_serializing_if`, so successful sessions and
/// pre-existing `result.toml` files (without the table) are unaffected.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PostExecGateReport {
    /// The gate command that was run (e.g. `"just pre-commit"`).
    pub gate_command: String,
    /// The gate's real exit code (e.g. `100`). A timeout/signal maps to a
    /// sentinel chosen by the caller.
    pub exit_code: i32,
    /// Best-effort recipe sub-step that failed (e.g. `"just test"`,
    /// `"just clippy"`). `None` when no failing step could be extracted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failing_step: Option<String>,
    /// Best-effort list of failed test names parsed from `cargo nextest`
    /// `FAIL [..] <test>` lines. Empty for non-test step failures.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failing_tests: Vec<String>,
    /// Bounded tail of the gate output (≤ [`GATE_OUTPUT_TAIL_MAX_LINES`] lines
    /// AND ≤ [`GATE_OUTPUT_TAIL_MAX_BYTES`] bytes). The complete, unbounded copy
    /// lives at [`PostExecGateReport::log_path`].
    pub output_tail: String,
    /// Relative path of the full gate output log
    /// ([`GATE_FAILURE_LOG_REL_PATH`]).
    pub log_path: String,
}

impl PostExecGateReport {
    /// Build a report from the gate command, exit code, and the gate's captured
    /// output.
    ///
    /// `redacted_output` MUST already be secret-redacted by the caller; the same
    /// redacted text is what should be written to [`GATE_FAILURE_LOG_REL_PATH`]
    /// so the bounded tail here can never expose material the log hides.
    pub fn from_redacted_gate_output(
        gate_command: &str,
        exit_code: i32,
        redacted_output: &str,
    ) -> Self {
        Self {
            gate_command: gate_command.to_string(),
            exit_code,
            failing_step: parse_failing_step(redacted_output),
            failing_tests: parse_nextest_failing_tests(redacted_output),
            output_tail: bound_output_tail(redacted_output),
            log_path: GATE_FAILURE_LOG_REL_PATH.to_string(),
        }
    }
}

/// Build the caller-facing one-line failure summary for a post-exec gate
/// report. It always leads with [`GATE_SUMMARY_LEAD`] and includes bounded
/// context only; the full output remains in [`PostExecGateReport::log_path`].
pub fn post_exec_gate_failure_summary(report: &PostExecGateReport) -> String {
    let mut summary = format!(
        "{GATE_SUMMARY_LEAD} (phase=post-exec, command={}, exit={}",
        report.gate_command, report.exit_code
    );
    if let Some(step) = &report.failing_step {
        summary.push_str(&format!(", step={step}"));
    }
    summary.push_str(") - employee self-report SUPERSEDED by gate verdict; full output: ");
    summary.push_str(&report.log_path);
    append_failing_tests(&mut summary, report);
    append_tail_excerpt(&mut summary, report);
    summary
}

/// Build a compact label for terminal summaries, e.g. the `csa session wait`
/// compact output.
pub fn post_exec_gate_failure_label(report: &PostExecGateReport) -> String {
    let mut label = format!(
        "failed (phase=post-exec, command={}, exit={}",
        report.gate_command, report.exit_code
    );
    if let Some(step) = &report.failing_step {
        label.push_str(&format!(", step={step}"));
    }
    label.push_str(&format!(", log={})", report.log_path));
    append_failing_tests(&mut label, report);
    label
}

fn append_failing_tests(message: &mut String, report: &PostExecGateReport) {
    if report.failing_tests.is_empty() {
        return;
    }
    let shown: Vec<&str> = report
        .failing_tests
        .iter()
        .take(SUMMARY_MAX_INLINE_TESTS)
        .map(String::as_str)
        .collect();
    message.push_str("; failing tests: ");
    message.push_str(&shown.join(", "));
    let extra = report
        .failing_tests
        .len()
        .saturating_sub(SUMMARY_MAX_INLINE_TESTS);
    if extra > 0 {
        message.push_str(&format!(" (+{extra} more)"));
    }
}

fn append_tail_excerpt(message: &mut String, report: &PostExecGateReport) {
    let Some(excerpt) = compact_tail_excerpt(&report.output_tail) else {
        return;
    };
    message.push_str("; tail: ");
    message.push_str(&excerpt);
}

fn compact_tail_excerpt(output_tail: &str) -> Option<String> {
    let line = output_tail
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let normalized = line.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(clip_chars(&normalized, SUMMARY_OUTPUT_EXCERPT_MAX_CHARS))
}

fn clip_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let truncated: String = value.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{}…", truncated.trim_end())
}

/// Parse failed test names from `cargo nextest` output.
///
/// nextest prints a failure summary line per failing test, formatted as
/// `FAIL [   0.005s] <test-id>` (the `[..]` holds the elapsed time). This
/// extracts `<test-id>` (everything after the `]`), de-duplicated and in first
/// occurrence order. Non-test gate output yields an empty list.
pub fn parse_nextest_failing_tests(output: &str) -> Vec<String> {
    let mut tests = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim_start();
        let Some(after_fail) = trimmed.strip_prefix("FAIL [") else {
            continue;
        };
        let Some(close_idx) = after_fail.find(']') else {
            continue;
        };
        let Some(test) = after_fail.get(close_idx + 1..).map(str::trim) else {
            continue;
        };
        if !test.is_empty() && !tests.iter().any(|existing| existing == test) {
            tests.push(test.to_string());
        }
    }
    tests
}

/// Best-effort extraction of the failing recipe sub-step.
///
/// `just` prints `error: Recipe `<name>` failed on line N with exit code M`
/// when a recipe fails; for a nested `just pre-commit` the innermost failing
/// recipe is printed first. The first such name is returned as `just <name>`
/// (e.g. `"just test"`, `"just clippy"`). Returns `None` when no recipe-failure
/// marker is present (e.g. a bare command gate).
pub fn parse_failing_step(output: &str) -> Option<String> {
    for line in output.lines() {
        let trimmed = line.trim();
        let Some(after_marker) = trimmed.strip_prefix("error: Recipe `") else {
            continue;
        };
        let Some(close_idx) = after_marker.find('`') else {
            continue;
        };
        if let Some(name) = after_marker.get(..close_idx)
            && !name.is_empty()
        {
            return Some(format!("just {name}"));
        }
    }
    None
}

/// Reduce gate output to a bounded tail for embedding in `result.toml`.
///
/// Keeps the last [`GATE_OUTPUT_TAIL_MAX_LINES`] lines, then further caps the
/// result to the last [`GATE_OUTPUT_TAIL_MAX_BYTES`] bytes (cutting on a UTF-8
/// char boundary) — "whichever is smaller". The unbounded output is expected to
/// be persisted to [`GATE_FAILURE_LOG_REL_PATH`] separately.
pub fn bound_output_tail(full: &str) -> String {
    let lines: Vec<&str> = full.split_inclusive('\n').collect();
    let start = lines.len().saturating_sub(GATE_OUTPUT_TAIL_MAX_LINES);
    let tail_by_lines = match lines.get(start..) {
        Some(slice) => slice.concat(),
        None => String::new(),
    };

    if tail_by_lines.len() <= GATE_OUTPUT_TAIL_MAX_BYTES {
        return tail_by_lines;
    }

    // Keep the LAST GATE_OUTPUT_TAIL_MAX_BYTES bytes, advancing to the next
    // char boundary so the slice is valid UTF-8.
    let mut cut = tail_by_lines.len() - GATE_OUTPUT_TAIL_MAX_BYTES;
    while cut < tail_by_lines.len() && !tail_by_lines.is_char_boundary(cut) {
        cut += 1;
    }
    tail_by_lines.get(cut..).unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nextest_fail_lines_into_test_names() {
        let output = "\
   Compiling csa-session v0.1.0
        PASS [   0.004s] csa-session result::tests::ok
        FAIL [   0.005s] csa-session result::tests::test_x
    FAIL [   1.200s] cli-sub-agent run_cmd::tests::test_y
   Summary [   1.234s] 2 tests run: 0 passed, 2 failed";
        let tests = parse_nextest_failing_tests(output);
        assert_eq!(
            tests,
            vec![
                "csa-session result::tests::test_x".to_string(),
                "cli-sub-agent run_cmd::tests::test_y".to_string(),
            ]
        );
    }

    #[test]
    fn dedupes_repeated_fail_lines() {
        let output = "FAIL [   0.005s] pkg::a\nFAIL [   0.006s] pkg::a\nFAIL [   0.007s] pkg::b";
        let tests = parse_nextest_failing_tests(output);
        assert_eq!(tests, vec!["pkg::a".to_string(), "pkg::b".to_string()]);
    }

    #[test]
    fn non_test_output_yields_no_failing_tests() {
        let output = "error: clippy failed\nwarning: unused variable\nerror[E0001]: bad";
        assert!(parse_nextest_failing_tests(output).is_empty());
    }

    #[test]
    fn parses_failing_step_from_just_recipe_error() {
        let output = "\
running cargo nextest run --workspace --all-features
        FAIL [   0.005s] csa-session result::tests::test_x
error: Recipe `test` failed on line 42 with exit code 100";
        assert_eq!(parse_failing_step(output).as_deref(), Some("just test"));
    }

    #[test]
    fn failing_step_picks_innermost_recipe_first() {
        let output = "\
error: Recipe `clippy` failed on line 10 with exit code 101
error: Recipe `pre-commit` failed on line 3 with exit code 101";
        assert_eq!(parse_failing_step(output).as_deref(), Some("just clippy"));
    }

    #[test]
    fn failing_step_absent_for_bare_command_gate() {
        let output = "error[E0308]: mismatched types\n  --> src/lib.rs:1:1";
        assert!(parse_failing_step(output).is_none());
    }

    #[test]
    fn clippy_failure_sets_step_but_no_tests() {
        // A non-test (clippy) step failure: failing_step is set, failing_tests
        // is empty, and output_tail is non-empty.
        let output = "\
    Checking cli-sub-agent v0.1.0
error: unused variable: `x`
  --> crates/cli-sub-agent/src/foo.rs:10:9
error: Recipe `clippy` failed on line 7 with exit code 101";
        let report = PostExecGateReport::from_redacted_gate_output("just pre-commit", 101, output);
        assert_eq!(report.failing_step.as_deref(), Some("just clippy"));
        assert!(report.failing_tests.is_empty());
        assert!(!report.output_tail.is_empty());
        assert_eq!(report.exit_code, 101);
        assert_eq!(report.gate_command, "just pre-commit");
        assert_eq!(report.log_path, GATE_FAILURE_LOG_REL_PATH);
    }

    #[test]
    fn bound_output_tail_keeps_full_short_output() {
        let output = "line 1\nline 2\nline 3";
        assert_eq!(bound_output_tail(output), output);
    }

    #[test]
    fn bound_output_tail_caps_to_last_lines() {
        let many: String = (0..500)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let tail = bound_output_tail(&many);
        let tail_lines: Vec<&str> = tail.lines().collect();
        assert_eq!(tail_lines.len(), GATE_OUTPUT_TAIL_MAX_LINES);
        // The cap keeps the LAST lines; the final line must be preserved.
        assert_eq!(tail_lines.last().copied(), Some("line 499"));
        assert_eq!(tail_lines.first().copied(), Some("line 400"));
    }

    #[test]
    fn bound_output_tail_caps_to_byte_budget() {
        // A single line longer than the byte budget is truncated to the last
        // GATE_OUTPUT_TAIL_MAX_BYTES bytes; the full content is what the caller
        // would write to gate-failure.log.
        let huge = "x".repeat(GATE_OUTPUT_TAIL_MAX_BYTES * 2);
        let tail = bound_output_tail(&huge);
        assert!(tail.len() <= GATE_OUTPUT_TAIL_MAX_BYTES);
        assert!(!tail.is_empty());
        // The tail is a suffix of the original content.
        assert!(huge.ends_with(&tail));
    }

    #[test]
    fn bound_output_tail_handles_multibyte_boundary() {
        // Multi-byte chars near the cut point must not panic or split a char.
        let unit = "🦀\n"; // 4-byte emoji + newline
        let huge = unit.repeat(GATE_OUTPUT_TAIL_MAX_BYTES);
        let tail = bound_output_tail(&huge);
        assert!(tail.len() <= GATE_OUTPUT_TAIL_MAX_BYTES);
        // Result is valid UTF-8 (no panic) and a suffix of the input.
        assert!(huge.ends_with(&tail));
    }

    #[test]
    fn empty_output_builds_empty_report_fields() {
        let report = PostExecGateReport::from_redacted_gate_output("just pre-commit", 100, "");
        assert_eq!(report.exit_code, 100);
        assert!(report.failing_step.is_none());
        assert!(report.failing_tests.is_empty());
        assert!(report.output_tail.is_empty());
        assert_eq!(report.log_path, GATE_FAILURE_LOG_REL_PATH);
    }

    #[test]
    fn report_roundtrips_as_toml_table() {
        let report = PostExecGateReport::from_redacted_gate_output(
            "just pre-commit",
            100,
            "FAIL [   0.005s] pkg::a\nerror: Recipe `test` failed on line 1 with exit code 100",
        );
        let toml_str = toml::to_string_pretty(&report).expect("serialize");
        let loaded: PostExecGateReport = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(loaded, report);
        assert_eq!(loaded.failing_tests, vec!["pkg::a".to_string()]);
        assert_eq!(loaded.failing_step.as_deref(), Some("just test"));
    }

    #[test]
    fn failure_summary_leads_with_gate_and_includes_bounded_tail() {
        let report = PostExecGateReport::from_redacted_gate_output(
            "post-exec gate",
            1,
            "FAIL [   0.005s] pkg::a\nerror: Recipe `test` failed on line 1 with exit code 100\n",
        );

        let summary = post_exec_gate_failure_summary(&report);

        assert!(summary.starts_with(GATE_SUMMARY_LEAD));
        assert!(summary.contains("phase=post-exec"));
        assert!(summary.contains("command=post-exec gate"));
        assert!(summary.contains("step=just test"));
        assert!(summary.contains("pkg::a"));
        assert!(summary.contains(GATE_FAILURE_LOG_REL_PATH));
        assert!(summary.contains("tail: error: Recipe `test` failed"));
    }

    #[test]
    fn failure_label_is_compact() {
        let report = PostExecGateReport::from_redacted_gate_output(
            "just pre-commit",
            100,
            "FAIL [   0.005s] pkg::a\nerror: Recipe `test` failed on line 1 with exit code 100\n",
        );

        let label = post_exec_gate_failure_label(&report);

        assert!(label.starts_with("failed (phase=post-exec"));
        assert!(label.contains("command=just pre-commit"));
        assert!(label.contains("step=just test"));
        assert!(label.contains(GATE_FAILURE_LOG_REL_PATH));
    }
}
