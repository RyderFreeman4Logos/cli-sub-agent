use std::fs;
use std::path::Path;
use std::str::FromStr;

use csa_core::types::{ReviewDecision, ToolName};
use csa_session::state::{ReviewSessionMeta, write_review_meta};
use csa_session::{Finding, ReviewArtifact, ReviewVerdictArtifact, write_review_verdict};
use csa_session::{output_parser::parse_sections, output_section::OutputSection};
use tracing::{debug, warn};

const REVIEW_RESULT_SUMMARY_MAX_CHARS: usize = 200;
const EDIT_RESTRICTION_SUMMARY_PREFIX: &str = "Edit restriction violated:";

#[derive(Debug, Clone)]
pub(super) struct ReviewerOutcome {
    pub reviewer_index: usize,
    pub tool: ToolName,
    pub session_id: String,
    pub output: String,
    pub exit_code: i32,
    pub verdict: &'static str,
    /// Tool-level diagnostic when the review failed due to tool issues (e.g. MCP).
    pub diagnostic: Option<String>,
}

/// Prefer structured review sections (summary/details) when available to avoid
/// leaking unrelated provider noise into caller-facing review output.
pub(super) fn sanitize_review_output(output: &str) -> String {
    let sections = parse_sections(output);
    if sections.is_empty() {
        return output.to_string();
    }

    let summary = last_non_empty_section_content(output, &sections, "summary");
    let details = last_non_empty_section_content(output, &sections, "details");
    if summary.is_none() && details.is_none() {
        return output.to_string();
    }

    let mut rendered = String::new();
    if let Some(content) = summary {
        rendered.push_str("<!-- CSA:SECTION:summary -->\n");
        rendered.push_str(&content);
        if !content.ends_with('\n') {
            rendered.push('\n');
        }
        rendered.push_str("<!-- CSA:SECTION:summary:END -->\n");
    }
    if let Some(content) = details {
        if !rendered.is_empty() && !rendered.ends_with('\n') {
            rendered.push('\n');
        }
        rendered.push_str("<!-- CSA:SECTION:details -->\n");
        rendered.push_str(&content);
        if !content.ends_with('\n') {
            rendered.push('\n');
        }
        rendered.push_str("<!-- CSA:SECTION:details:END -->\n");
    }
    rendered
}

pub(super) fn has_structured_review_content(output: &str) -> bool {
    let sanitized = sanitize_review_output(output);
    let sections = parse_sections(&sanitized);
    ["summary", "details"].into_iter().any(|section_id| {
        last_non_empty_section_content(&sanitized, &sections, section_id).is_some()
    })
}

pub(super) fn derive_review_result_summary(output: &str) -> Option<String> {
    let sanitized = sanitize_review_output(output);
    let sections = parse_sections(&sanitized);
    let content = last_non_empty_section_content(&sanitized, &sections, "summary")
        .or_else(|| last_non_empty_section_content(&sanitized, &sections, "details"))?;

    content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(truncate_review_result_summary)
}

pub(super) fn is_edit_restriction_summary(summary: &str) -> bool {
    summary.starts_with(EDIT_RESTRICTION_SUMMARY_PREFIX)
}

fn last_non_empty_section_content(
    output: &str,
    sections: &[OutputSection],
    section_id: &str,
) -> Option<String> {
    sections
        .iter()
        .rev()
        .filter(|section| section.id == section_id)
        .find_map(|section| {
            let content = extract_section_content(output, section);
            if content.trim().is_empty() {
                None
            } else {
                Some(content)
            }
        })
}

fn extract_section_content(output: &str, section: &OutputSection) -> String {
    if section.line_start == 0 || section.line_end < section.line_start {
        return String::new();
    }

    let lines: Vec<&str> = output.lines().collect();
    if lines.is_empty() || section.line_start > lines.len() {
        return String::new();
    }

    let start = section.line_start - 1;
    let end_exclusive = section.line_end.min(lines.len());
    lines[start..end_exclusive].join("\n")
}

/// Persist a [`ReviewSessionMeta`] to `{session_dir}/review_meta.json`.
///
/// Best-effort: failures are logged as warnings but do not fail the review.
pub(super) fn persist_review_meta(project_root: &Path, meta: &ReviewSessionMeta) {
    match csa_session::get_session_dir(project_root, &meta.session_id) {
        Ok(session_dir) => {
            if let Err(e) = write_review_meta(&session_dir, meta) {
                warn!(session_id = %meta.session_id, error = %e, "Failed to write review_meta.json");
            } else {
                debug!(session_id = %meta.session_id, "Wrote review_meta.json");
            }
        }
        Err(e) => {
            warn!(session_id = %meta.session_id, error = %e, "Cannot resolve session dir for review meta");
        }
    }
}

/// Persist a [`ReviewVerdictArtifact`] to `{session_dir}/output/review-verdict.json`.
///
/// Best-effort: failures are logged as warnings but do not fail the review.
pub(super) fn persist_review_verdict(
    project_root: &Path,
    meta: &ReviewSessionMeta,
    findings: &[Finding],
    prior_round_refs: Vec<String>,
) {
    match csa_session::get_session_dir(project_root, &meta.session_id) {
        Ok(session_dir) => {
            let verdict_path = session_dir.join("output").join("review-verdict.json");
            if verdict_path.exists() {
                debug!(
                    session_id = %meta.session_id,
                    path = %verdict_path.display(),
                    "Skipping output/review-verdict.json persistence because AI artifact already exists"
                );
                return;
            }
            let decision =
                ReviewDecision::from_str(&meta.decision).unwrap_or(ReviewDecision::Uncertain);
            let synthesized_findings = match load_review_findings_from_output(&session_dir) {
                Ok(Some(loaded_findings)) => loaded_findings,
                Ok(None) => findings.to_vec(),
                Err(error) => {
                    debug!(
                        session_id = %meta.session_id,
                        error = %error,
                        "Failed to load output/review-findings.json; synthesizing empty review-verdict sidecar"
                    );
                    findings.to_vec()
                }
            };
            let artifact = ReviewVerdictArtifact::from_parts(
                meta.session_id.clone(),
                decision,
                meta.verdict.clone(),
                &synthesized_findings,
                prior_round_refs,
            );
            if let Err(e) = write_review_verdict(&session_dir, &artifact) {
                warn!(
                    session_id = %meta.session_id,
                    error = %e,
                    "Failed to write output/review-verdict.json"
                );
            } else {
                debug!(session_id = %meta.session_id, "Wrote output/review-verdict.json");
            }
        }
        Err(e) => {
            warn!(
                session_id = %meta.session_id,
                error = %e,
                "Cannot resolve session dir for review verdict"
            );
        }
    }
}

fn load_review_findings_from_output(
    session_dir: &Path,
) -> Result<Option<Vec<Finding>>, anyhow::Error> {
    let findings_path = session_dir.join("review-findings.json");
    if !findings_path.exists() {
        return Ok(None);
    }

    let contents = fs::read_to_string(&findings_path)
        .map_err(|error| anyhow::anyhow!("read {}: {error}", findings_path.display()))?;
    let artifact = serde_json::from_str::<ReviewArtifact>(&contents)
        .map_err(|error| anyhow::anyhow!("parse {}: {error}", findings_path.display()))?;
    Ok(Some(artifact.findings))
}

/// Detect whether `project_root` resides inside a git worktree submodule.
///
/// A worktree submodule's `.git` is a file (not directory) containing a
/// `gitdir:` reference that traverses both `worktrees/` and `modules/`
/// path segments — the hallmark of the nested worktree-submodule layout.
pub(super) fn is_worktree_submodule(project_root: &Path) -> bool {
    let git_marker = project_root.join(".git");
    if !git_marker.is_file() {
        return false;
    }
    let Ok(marker) = std::fs::read_to_string(&git_marker) else {
        return false;
    };
    let Some(gitdir_raw) = marker.trim().strip_prefix("gitdir:") else {
        return false;
    };
    let gitdir = gitdir_raw.trim();
    gitdir.contains("/worktrees/") && gitdir.contains("/modules/")
}

/// Detect known tool-level diagnostic messages that indicate the review tool
/// failed to actually perform a review (e.g., gemini-cli MCP connectivity issues).
///
/// Checks both stdout and stderr for known failure patterns.
/// Returns a human-readable diagnostic summary when a known pattern is found.
pub(super) fn detect_tool_diagnostic(stdout: &str, stderr: &str) -> Option<String> {
    let has_mcp_issue =
        |text: &str| text.contains("MCP issues detected") || text.contains("Run /mcp list");

    if has_mcp_issue(stdout) || has_mcp_issue(stderr) {
        return Some(
            "gemini-cli encountered MCP server connectivity issues. \
             Run `gemini /mcp list` to diagnose. \
             Consider using `--tool claude-code` as a fallback."
                .to_string(),
        );
    }

    None
}

/// Print per-reviewer output and diagnostics for multi-reviewer mode.
pub(super) fn print_reviewer_outcomes(outcomes: &[ReviewerOutcome]) {
    for o in outcomes {
        let r = o.reviewer_index + 1;
        println!(
            "===== Reviewer {r} ({}) | verdict={} | exit_code={} =====",
            o.tool, o.verdict, o.exit_code
        );
        if let Some(ref d) = o.diagnostic {
            eprintln!("[csa-review] Reviewer {r} tool failure: {d}");
        }
        print!("{}", o.output);
        if !o.output.ends_with('\n') {
            println!();
        }
    }
}

/// Check whether review output contains substantive content beyond prompt guards.
///
/// Returns `true` when the raw output is empty or contains only CSA prompt
/// injection markers / hook output and whitespace — indicating the review tool
/// produced no actual findings.
pub(super) fn is_review_output_empty(raw_output: &str) -> bool {
    strip_prompt_guards(raw_output).trim().is_empty()
}

/// Remove non-review content: prompt injection blocks, hook markers, and section wrappers.
fn strip_prompt_guards(text: &str) -> String {
    let mut result = String::new();
    let mut in_guard = false;
    for line in text.lines() {
        if line.contains("<csa-caller-prompt-injection") {
            in_guard = true;
            continue;
        }
        if line.contains("</csa-caller-prompt-injection>") {
            in_guard = false;
            continue;
        }
        if in_guard {
            continue;
        }
        if line.trim_start().starts_with("[csa-hook]") {
            continue;
        }
        if line.trim_start().starts_with("[csa-heartbeat]") {
            continue;
        }
        // Strip CSA section markers (empty wrappers are not substantive content)
        if line.trim_start().starts_with("<!-- CSA:SECTION:") {
            continue;
        }
        result.push_str(line);
        result.push('\n');
    }
    result
}

fn truncate_review_result_summary(line: &str) -> String {
    line.chars().take(REVIEW_RESULT_SUMMARY_MAX_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::persist_review_verdict;
    use csa_core::types::ReviewDecision;
    use csa_session::state::ReviewSessionMeta;
    use csa_session::{Finding, ReviewArtifact, ReviewVerdictArtifact, Severity, SeveritySummary};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn make_review_meta(session_id: &str) -> ReviewSessionMeta {
        ReviewSessionMeta {
            session_id: session_id.to_string(),
            head_sha: String::new(),
            decision: ReviewDecision::Fail.as_str().to_string(),
            verdict: "HAS_ISSUES".to_string(),
            tool: "codex".to_string(),
            scope: "diff".to_string(),
            exit_code: 1,
            fix_attempted: false,
            fix_rounds: 0,
            review_iterations: 1,
            timestamp: chrono::Utc::now(),
            diff_fingerprint: None,
        }
    }

    fn make_finding(severity: Severity, fid: &str) -> Finding {
        Finding {
            severity,
            fid: fid.to_string(),
            file: "src/lib.rs".to_string(),
            line: Some(1),
            rule_id: format!("rule.{fid}"),
            summary: format!("summary {fid}"),
            engine: "reviewer".to_string(),
        }
    }

    fn temp_project_root(test_name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("csa-{test_name}-{suffix}"));
        fs::create_dir_all(&path).expect("create temp project root");
        path
    }

    fn create_session_dir(project_root: &Path, session_id: &str) -> PathBuf {
        let session_dir =
            csa_session::get_session_dir(project_root, session_id).expect("resolve session dir");
        fs::create_dir_all(session_dir.join("output")).expect("create session output dir");
        session_dir
    }

    #[test]
    fn persist_review_verdict_skips_when_ai_file_exists() {
        let project_root = temp_project_root("persist-review-verdict-skip");
        let session_id = "01TESTSKIP0000000000000000";
        let session_dir = create_session_dir(&project_root, session_id);
        let verdict_path = session_dir.join("output").join("review-verdict.json");
        let ai_payload = r#"{"ai":"preserved"}"#;
        fs::write(&verdict_path, ai_payload).expect("write AI verdict artifact");

        let meta = make_review_meta(session_id);
        persist_review_verdict(&project_root, &meta, &[], Vec::new());

        let actual = fs::read_to_string(&verdict_path).expect("read verdict artifact");
        assert_eq!(actual, ai_payload);

        fs::remove_dir_all(project_root).expect("remove temp project root");
    }

    #[test]
    fn persist_review_verdict_synthesizes_from_findings_json() {
        let project_root = temp_project_root("persist-review-verdict-findings");
        let session_id = "01TESTFINDINGS000000000000";
        let session_dir = create_session_dir(&project_root, session_id);
        let findings_path = session_dir.join("review-findings.json");
        let findings = vec![
            make_finding(Severity::High, "high"),
            make_finding(Severity::Low, "low"),
        ];
        let artifact = ReviewArtifact {
            severity_summary: SeveritySummary::from_findings(&findings),
            findings: findings.clone(),
            review_mode: None,
            schema_version: "1.0".to_string(),
            session_id: session_id.to_string(),
            timestamp: chrono::Utc::now(),
        };
        fs::write(
            &findings_path,
            serde_json::to_vec_pretty(&artifact).expect("serialize findings"),
        )
        .expect("write findings artifact");

        let meta = make_review_meta(session_id);
        persist_review_verdict(&project_root, &meta, &[], Vec::new());

        let verdict_path = session_dir.join("output").join("review-verdict.json");
        let artifact: ReviewVerdictArtifact =
            serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
                .expect("parse verdict");
        assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&1));
        assert_eq!(artifact.severity_counts.get(&Severity::Low), Some(&1));

        fs::remove_dir_all(project_root).expect("remove temp project root");
    }

    #[test]
    fn persist_review_verdict_empty_sidecar_when_findings_missing() {
        let project_root = temp_project_root("persist-review-verdict-empty");
        let session_id = "01TESTEMPTY000000000000000";
        let session_dir = create_session_dir(&project_root, session_id);
        let meta = make_review_meta(session_id);

        persist_review_verdict(&project_root, &meta, &[], Vec::new());

        let verdict_path = session_dir.join("output").join("review-verdict.json");
        let artifact: ReviewVerdictArtifact =
            serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
                .expect("parse verdict");
        assert_eq!(artifact.severity_counts.len(), 5);
        assert!(artifact.severity_counts.values().all(|value| *value == 0));

        fs::remove_dir_all(project_root).expect("remove temp project root");
    }
}
