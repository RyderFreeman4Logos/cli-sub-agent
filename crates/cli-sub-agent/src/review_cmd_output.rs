use std::path::Path;

use csa_core::types::ToolName;
use csa_session::state::{ReviewSessionMeta, write_review_meta};
use csa_session::{output_parser::parse_sections, output_section::OutputSection};
use tracing::{debug, warn};

#[derive(Debug, Clone)]
pub(super) struct ReviewerOutcome {
    pub reviewer_index: usize,
    pub tool: ToolName,
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
