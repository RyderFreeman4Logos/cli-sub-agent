//! Structured surfacing of post-exec gate failures (#1726).
//!
//! When a `csa run` write session's post-exec verification gate (e.g.
//! `just pre-commit`) exits nonzero, the gate's stdout/stderr previously landed
//! ONLY in the raw transcript `output/full.md`. An SA-Layer-0 orchestrator —
//! which is forbidden from reading transcripts — could not diagnose the failing
//! step/test, and `summary.md`/`details.md` (written from the employee's
//! pre-gate self-report) could still read as "success" while `result.toml` said
//! the gate failed.
//!
//! This module reconciles that. On a nonzero-exit gate failure it:
//!  - writes the full (redacted) gate output to [`GATE_FAILURE_LOG_REL_PATH`];
//!  - overwrites `result.toml` so its `summary` LEADS with the gate verdict and
//!    its `[post_exec_gate]` table carries a bounded tail plus the parsed
//!    failing step/tests;
//!  - prepends a gate-failure banner to `summary.md`/`details.md` so the
//!    employee's pre-gate self-report can never read as the final verdict.
//!
//! It deliberately covers ONLY the nonzero-exit path. Timeout and gate
//! infrastructure errors keep the existing simple overwrite in
//! [`crate::run_cmd_post`]: their verdicts are already non-contradictory and
//! they carry no captured gate output worth surfacing structurally.

use std::path::Path;

use tracing::warn;

use csa_session::{
    GATE_FAILURE_LOG_REL_PATH, PostExecGateReport, get_session_dir, load_result,
    redact_text_content, save_result,
};

/// Stable banner header prefix; used both to render the banner and to detect an
/// already-prepended banner (idempotency) so re-entry never double-stamps.
const GATE_BANNER_HEADER: &str = "> ⚠️ **POST-EXEC GATE FAILED";

/// How many failing test names to list in the (multi-line) section banner.
const BANNER_MAX_LISTED_TESTS: usize = 10;

/// Inputs for [`persist_gate_failure_detail`].
pub(crate) struct GateFailureDetail<'a> {
    pub(crate) project_root: &'a Path,
    pub(crate) session_id: &'a str,
    /// The gate command that failed (e.g. `"just pre-commit"`).
    pub(crate) gate_command: &'a str,
    /// The gate's real exit code (signal → `-1`, timeout → `124` by convention).
    pub(crate) exit_code: i32,
    /// Full captured gate output, BEFORE redaction.
    pub(crate) captured_output: &'a str,
}

/// Persist a nonzero-exit post-exec gate failure into structured, SA-readable
/// artifacts and reconcile the session's summary so it leads with the gate
/// verdict. Best-effort throughout: each sub-step logs and continues on error so
/// a partial failure (e.g. an unwritable section file) never masks the gate
/// verdict already recorded in `result.toml`.
pub(crate) fn persist_gate_failure_detail(detail: GateFailureDetail<'_>) {
    // Redact ONCE; the same redacted text feeds both the unbounded log and the
    // bounded tail in the report, so neither surface can leak material the other
    // hides.
    let redacted = redact_text_content(detail.captured_output);
    let report = PostExecGateReport::from_redacted_gate_output(
        detail.gate_command,
        detail.exit_code,
        &redacted,
    );

    // Resolve the session dir for the log + section banners. If it cannot be
    // resolved we still overwrite result.toml below (it resolves its own path),
    // so the authoritative verdict is recorded even without the log/banners.
    let session_dir = match get_session_dir(detail.project_root, detail.session_id) {
        Ok(dir) => Some(dir),
        Err(err) => {
            warn!(
                session = %detail.session_id,
                error = %err,
                "Could not resolve session dir for gate-failure surfacing; recording verdict only"
            );
            None
        }
    };

    if let Some(dir) = session_dir.as_deref() {
        write_gate_failure_log(dir, detail.session_id, &redacted);
    }

    overwrite_result_with_report(detail.project_root, detail.session_id, &report);

    if let Some(dir) = session_dir.as_deref() {
        prepend_gate_banner_to_sections(dir, detail.session_id, &report);
    }

    // Retire so the dead session isn't left Active (matches the simple overwrite
    // path in run_cmd_post).
    if let Err(err) = crate::run_cmd_post::retire_session_after_gate_failure(
        detail.project_root,
        detail.session_id,
    ) {
        warn!(
            session = %detail.session_id,
            error = %err,
            "Failed to retire session after post-exec gate failure"
        );
    }
}

/// Write the full (already-redacted) gate output to `output/gate-failure.log`.
fn write_gate_failure_log(session_dir: &Path, session_id: &str, redacted_output: &str) {
    let log_path = session_dir.join(GATE_FAILURE_LOG_REL_PATH);
    if let Some(parent) = log_path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        warn!(
            session = %session_id,
            error = %err,
            "Could not create output dir for gate-failure.log"
        );
        return;
    }
    if let Err(err) = std::fs::write(&log_path, redacted_output) {
        warn!(
            session = %session_id,
            error = %err,
            "Could not write gate-failure.log"
        );
    }
}

/// Overwrite `result.toml`: the gate verdict is authoritative over the
/// employee's pre-gate self-report.
fn overwrite_result_with_report(
    project_root: &Path,
    session_id: &str,
    report: &PostExecGateReport,
) {
    match load_result(project_root, session_id) {
        Ok(Some(mut result)) => {
            result.exit_code = 1;
            result.status = "failure".to_string();
            // A nonzero-exit gate is not a timeout.
            result.gate_timeout = false;
            result.summary = build_failure_summary(report);
            result.post_exec_gate = Some(report.clone());
            if let Err(err) = save_result(project_root, session_id, &result) {
                warn!(
                    session = %session_id,
                    error = %err,
                    "Failed to overwrite result.toml after post-exec gate failure"
                );
            }
        }
        Ok(None) => warn!(
            session = %session_id,
            "No result.toml to overwrite after post-exec gate failure"
        ),
        Err(err) => warn!(
            session = %session_id,
            error = %err,
            "Failed to load result.toml for post-exec gate failure overwrite"
        ),
    }
}

/// Build the one-line `result.toml` summary. It LEADS with the gate verdict
/// (exit code + failing step) and points at the full log, so an orchestrator
/// reading only `result.summary` cannot mistake the run for a success.
fn build_failure_summary(report: &PostExecGateReport) -> String {
    csa_session::post_exec_gate_failure_summary(report)
}

/// Build the multi-line markdown banner prepended to `summary.md`/`details.md`.
fn build_gate_failure_banner(report: &PostExecGateReport) -> String {
    let mut banner = String::new();
    banner.push_str(GATE_BANNER_HEADER);
    banner.push_str(
        " — the employee self-report below is SUPERSEDED by the verification gate.**\n>\n",
    );
    banner.push_str(&format!("> - Gate command: `{}`\n", report.gate_command));
    banner.push_str(&format!("> - Exit code: `{}`\n", report.exit_code));
    if let Some(step) = &report.failing_step {
        banner.push_str(&format!("> - Failing step: `{step}`\n"));
    }
    if !report.failing_tests.is_empty() {
        let shown: Vec<&str> = report
            .failing_tests
            .iter()
            .take(BANNER_MAX_LISTED_TESTS)
            .map(String::as_str)
            .collect();
        banner.push_str(&format!("> - Failing tests: {}", shown.join(", ")));
        let extra = report
            .failing_tests
            .len()
            .saturating_sub(BANNER_MAX_LISTED_TESTS);
        if extra > 0 {
            banner.push_str(&format!(" (+{extra} more)"));
        }
        banner.push('\n');
    }
    banner.push_str(&format!("> - Full gate output: `{}`\n", report.log_path));
    banner.push_str(
        ">\n> The session did NOT pass verification. Treat any \"success\" / \"all clean\" \
         wording below as the employee's pre-gate claim, not the final verdict.\n\n---\n\n",
    );
    banner
}

/// Prepend the gate-failure banner to `summary.md` (creating it if the employee
/// emitted none) and to `details.md` (only if it already exists). Idempotent:
/// a file already starting with the banner header is left untouched.
fn prepend_gate_banner_to_sections(
    session_dir: &Path,
    session_id: &str,
    report: &PostExecGateReport,
) {
    let banner = build_gate_failure_banner(report);
    let output_dir = session_dir.join("output");

    // summary.md is the canonical SA-read surface, so it must ALWAYS lead with
    // the gate verdict — create it with the banner when the employee emitted
    // none.
    let summary_path = output_dir.join("summary.md");
    let existing_summary = std::fs::read_to_string(&summary_path).unwrap_or_default();
    if !existing_summary
        .trim_start()
        .starts_with(GATE_BANNER_HEADER)
    {
        if let Err(err) = std::fs::create_dir_all(&output_dir) {
            warn!(
                session = %session_id,
                error = %err,
                "Could not create output dir for gate-failure banner"
            );
            return;
        }
        if let Err(err) = std::fs::write(&summary_path, format!("{banner}{existing_summary}")) {
            warn!(
                session = %session_id,
                error = %err,
                "Could not prepend gate-failure banner to summary.md"
            );
        }
    }

    // details.md retains the employee's account but must be prefixed with the
    // banner; only touch it when it already exists.
    let details_path = output_dir.join("details.md");
    if details_path.is_file() {
        let existing_details = std::fs::read_to_string(&details_path).unwrap_or_default();
        if !existing_details
            .trim_start()
            .starts_with(GATE_BANNER_HEADER)
            && let Err(err) = std::fs::write(&details_path, format!("{banner}{existing_details}"))
        {
            warn!(
                session = %session_id,
                error = %err,
                "Could not prepend gate-failure banner to details.md"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report_with(
        exit_code: i32,
        failing_step: Option<&str>,
        failing_tests: &[&str],
    ) -> PostExecGateReport {
        PostExecGateReport {
            gate_command: "just pre-commit".to_string(),
            exit_code,
            failing_step: failing_step.map(str::to_string),
            failing_tests: failing_tests.iter().map(|t| t.to_string()).collect(),
            output_tail: "…tail…".to_string(),
            log_path: GATE_FAILURE_LOG_REL_PATH.to_string(),
        }
    }

    #[test]
    fn summary_leads_with_gate_verdict_not_success() {
        let summary = build_failure_summary(&report_with(100, Some("just test"), &["pkg::a"]));
        // Leads with the gate verdict marker.
        assert!(summary.starts_with(csa_session::GATE_SUMMARY_LEAD));
        assert!(summary.contains("exit=100"));
        assert!(summary.contains("step=just test"));
        assert!(summary.contains("SUPERSEDED"));
        assert!(summary.contains(GATE_FAILURE_LOG_REL_PATH));
        assert!(summary.contains("pkg::a"));
    }

    #[test]
    fn summary_handles_missing_step_and_tests() {
        let summary = build_failure_summary(&report_with(1, None, &[]));
        assert!(summary.starts_with(csa_session::GATE_SUMMARY_LEAD));
        assert!(summary.contains("exit=1"));
        assert!(!summary.contains("step="));
        assert!(!summary.contains("failing tests"));
    }

    #[test]
    fn summary_collapses_overflow_tests_into_more_suffix() {
        let many = ["a", "b", "c", "d", "e", "f", "g"];
        let summary = build_failure_summary(&report_with(100, None, &many));
        // First SUMMARY_MAX_INLINE_TESTS shown, remainder collapsed.
        assert!(summary.contains("a, b, c, d, e"));
        assert!(!summary.contains(", f"));
        assert!(summary.contains("(+2 more)"));
    }

    #[test]
    fn banner_supersedes_and_lists_failure_detail() {
        let banner = build_gate_failure_banner(&report_with(101, Some("just clippy"), &["pkg::x"]));
        assert!(banner.trim_start().starts_with(GATE_BANNER_HEADER));
        assert!(banner.contains("SUPERSEDED"));
        assert!(banner.contains("just pre-commit"));
        assert!(banner.contains("`101`"));
        assert!(banner.contains("just clippy"));
        assert!(banner.contains("pkg::x"));
        assert!(banner.contains(GATE_FAILURE_LOG_REL_PATH));
        // Ends with a markdown rule separating it from the employee account.
        assert!(banner.contains("\n---\n"));
    }

    #[test]
    fn banner_caps_listed_tests() {
        let many: Vec<String> = (0..15).map(|i| format!("pkg::t{i}")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        let banner = build_gate_failure_banner(&report_with(100, None, &refs));
        assert!(banner.contains("pkg::t0"));
        assert!(banner.contains("pkg::t9"));
        // 11th listed test is collapsed.
        assert!(!banner.contains("pkg::t10,"));
        assert!(banner.contains("(+5 more)"));
    }
}
