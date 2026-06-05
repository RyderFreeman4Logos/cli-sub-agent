use std::collections::BTreeSet;
use std::path::{Component, Path};
use std::process::Command;

use csa_config::{GlobalConfig, ProjectConfig, ReviewConfig};
use csa_session::state::{ReviewSessionMeta, write_review_meta};
use csa_session::{ReviewDiffSize, ReviewVerdictArtifact, write_review_verdict};
use tracing::{debug, warn};

// #1841 cross-dimension breadth-first blocking enumeration lives in its own file
// so this #1645 diff-size module stays within the per-module token budget.
#[path = "review_cmd_enumeration_mode.rs"]
mod enumeration_mode;
pub(super) use enumeration_mode::append_cross_dimension_anchor;

const REVIEW_DIFF_SIZE_LINE_PREFIX: &str = "Diff size:";
const REVIEW_DIFF_SIZE_HEADER_SECTION_IDS: &[&str] = &["summary", "details"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LargeDiffWarning {
    pub(super) changed_lines: usize,
    pub(super) threshold: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ReviewDiffReport<'a> {
    pub(super) diff_size: Option<&'a ReviewDiffSize>,
    pub(super) large_diff_warning: Option<LargeDiffWarning>,
}

impl LargeDiffWarning {
    fn message(self) -> String {
        format!(
            "review diff is large ({} changed lines > review.large_diff_warn_lines={}); single-reviewer coverage confidence may be reduced; consider heterogeneous/chunked review (#1645)",
            self.changed_lines, self.threshold
        )
    }
}

pub(super) fn compute_review_diff_size(project_root: &Path, scope: &str) -> Option<ReviewDiffSize> {
    if scope == "uncommitted" {
        return compute_uncommitted_review_diff_size(project_root);
    }

    let diff = collect_review_diff_payload(project_root, scope)?;
    Some(diff_size_from_payload(&diff))
}

pub(super) fn resolve_large_diff_warn_lines(
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
) -> Option<usize> {
    project_config
        .and_then(|config| config.review.as_ref())
        .and_then(|review| review.large_diff_warn_lines)
        .or(global_config.review.large_diff_warn_lines)
        .or_else(ReviewConfig::default_large_diff_warn_lines)
}

pub(super) fn large_diff_warning(
    diff_size: &ReviewDiffSize,
    threshold: Option<usize>,
) -> Option<LargeDiffWarning> {
    let threshold = threshold?;
    if threshold == 0 || diff_size.changed_lines <= threshold {
        return None;
    }
    Some(LargeDiffWarning {
        changed_lines: diff_size.changed_lines,
        threshold,
    })
}

pub(super) fn emit_large_diff_warning(warning: LargeDiffWarning) {
    eprintln!("{}", format_large_diff_warning(warning));
}

/// Resolve the #1645 large-diff warning for an already-sized review diff and emit
/// it to the operator (stderr) when the diff exceeds the configured threshold.
/// Returns the warning (when present) so the caller can still thread it into
/// downstream review artifacts. Pure extraction of the former inline command-layer
/// block; behavior is unchanged.
pub(super) fn warn_if_large_diff(
    diff_size: Option<&ReviewDiffSize>,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
) -> Option<LargeDiffWarning> {
    let warning = diff_size.and_then(|diff_size| {
        large_diff_warning(
            diff_size,
            resolve_large_diff_warn_lines(project_config, global_config),
        )
    });
    if let Some(warning) = warning {
        emit_large_diff_warning(warning);
    }
    warning
}

pub(super) fn format_large_diff_warning(warning: LargeDiffWarning) -> String {
    format!("warning: {}", warning.message())
}

pub(super) fn format_review_diff_size_line(diff_size: &ReviewDiffSize) -> String {
    let mut line = format!(
        "{REVIEW_DIFF_SIZE_LINE_PREFIX} {} files, {} changed lines, {} bytes",
        diff_size.files, diff_size.changed_lines, diff_size.bytes
    );
    if !diff_size.notes.is_empty() {
        line.push_str("; ");
        line.push_str(&diff_size.notes.join("; "));
    }
    line
}

pub(super) fn add_review_diff_size_line(
    output: &str,
    diff_size: Option<&ReviewDiffSize>,
) -> String {
    let Some(diff_size) = diff_size else {
        return output.to_string();
    };
    let line = format_review_diff_size_line(diff_size);
    if has_review_diff_size_header(output) {
        return output.to_string();
    }
    if output.is_empty() {
        return format!("{line}\n");
    }
    format!("{line}\n{output}")
}

pub(super) fn persist_review_diff_size_headers(
    project_root: &Path,
    session_id: &str,
    diff_size: Option<&ReviewDiffSize>,
) {
    let Some(diff_size) = diff_size else {
        return;
    };
    let Ok(session_dir) = csa_session::get_session_dir(project_root, session_id) else {
        return;
    };
    let line = format_review_diff_size_line(diff_size);
    let output_dir = session_dir.join("output");
    for file_name in review_diff_size_header_file_paths(&session_dir) {
        let path = output_dir.join(file_name);
        let Ok(existing) = std::fs::read_to_string(&path) else {
            continue;
        };
        if has_review_diff_size_header(&existing) {
            continue;
        }
        if let Err(error) = std::fs::write(&path, format!("{line}\n{existing}")) {
            debug!(
                session_id,
                file = %path.display(),
                error = %error,
                "Failed to write review diff-size header"
            );
        }
    }
}

fn has_review_diff_size_header(output: &str) -> bool {
    output
        .lines()
        .next()
        .is_some_and(|line| line.starts_with(REVIEW_DIFF_SIZE_LINE_PREFIX))
}

fn review_diff_size_header_file_paths(session_dir: &Path) -> BTreeSet<String> {
    match csa_session::load_output_index(session_dir) {
        Ok(Some(index)) => index
            .sections
            .into_iter()
            .filter(|section| REVIEW_DIFF_SIZE_HEADER_SECTION_IDS.contains(&section.id.as_str()))
            .filter_map(|section| section.file_path)
            .filter(|file_path| is_output_file_name(file_path))
            .collect(),
        Ok(None) => legacy_review_diff_size_header_file_paths(),
        Err(error) => {
            debug!(
                session_dir = %session_dir.display(),
                error = %error,
                "Failed to read output index for review diff-size headers"
            );
            legacy_review_diff_size_header_file_paths()
        }
    }
}

fn legacy_review_diff_size_header_file_paths() -> BTreeSet<String> {
    REVIEW_DIFF_SIZE_HEADER_SECTION_IDS
        .iter()
        .map(|section_id| format!("{section_id}.md"))
        .collect()
}

fn is_output_file_name(file_path: &str) -> bool {
    let path = Path::new(file_path);
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

pub(super) fn persist_review_meta_with_diff_report(
    project_root: &Path,
    meta: &ReviewSessionMeta,
    diff_size: Option<&ReviewDiffSize>,
    large_diff_warning: Option<LargeDiffWarning>,
) {
    match csa_session::get_session_dir(project_root, &meta.session_id) {
        Ok(session_dir) => {
            if let Err(error) = write_review_meta_with_diff_report(
                &session_dir,
                meta,
                diff_size,
                large_diff_warning,
            ) {
                warn!(
                    session_id = %meta.session_id,
                    error = %error,
                    "Failed to write review_meta.json"
                );
            }
        }
        Err(error) => {
            warn!(
                session_id = %meta.session_id,
                error = %error,
                "Cannot resolve session dir for review meta"
            );
        }
    }
}

pub(super) fn write_review_meta_with_diff_report(
    session_dir: &Path,
    meta: &ReviewSessionMeta,
    diff_size: Option<&ReviewDiffSize>,
    large_diff_warning: Option<LargeDiffWarning>,
) -> std::io::Result<()> {
    if diff_size.is_none() && large_diff_warning.is_none() {
        return write_review_meta(session_dir, meta);
    }

    let path = session_dir.join("review_meta.json");
    let mut value = serde_json::to_value(meta).map_err(std::io::Error::other)?;
    if let Some(object) = value.as_object_mut() {
        if let Some(diff_size) = diff_size {
            object.insert(
                "diff_size".to_string(),
                serde_json::to_value(diff_size).map_err(std::io::Error::other)?,
            );
        }
        insert_large_diff_warning_fields(object, large_diff_warning);
    }
    let json = serde_json::to_string_pretty(&value).map_err(std::io::Error::other)?;
    std::fs::write(path, json)
}

pub(super) fn persist_review_verdict_diff_report(
    project_root: &Path,
    session_id: &str,
    artifact: &mut ReviewVerdictArtifact,
    diff_size: Option<&ReviewDiffSize>,
    large_diff_warning: Option<LargeDiffWarning>,
) {
    if diff_size.is_none() && large_diff_warning.is_none() {
        return;
    }
    if let Some(diff_size) = diff_size {
        artifact.diff_size = Some((*diff_size).clone());
    }
    apply_large_diff_warning(artifact, large_diff_warning);
    let Ok(session_dir) = csa_session::get_session_dir(project_root, session_id) else {
        return;
    };
    if let Err(error) = write_review_verdict(&session_dir, artifact) {
        warn!(
            session_id,
            error = %error,
            "Failed to rewrite review-verdict.json with review diff report"
        );
    }
}

pub(super) fn apply_large_diff_warning(
    artifact: &mut ReviewVerdictArtifact,
    large_diff_warning: Option<LargeDiffWarning>,
) {
    if let Some(warning) = large_diff_warning {
        artifact.large_diff_warning = true;
        artifact.large_diff_warning_threshold = Some(warning.threshold);
        artifact.large_diff_warning_changed_lines = Some(warning.changed_lines);
    }
}

fn insert_large_diff_warning_fields(
    object: &mut serde_json::Map<String, serde_json::Value>,
    large_diff_warning: Option<LargeDiffWarning>,
) {
    if let Some(warning) = large_diff_warning {
        object.insert(
            "large_diff_warning".to_string(),
            serde_json::Value::Bool(true),
        );
        object.insert(
            "large_diff_warning_threshold".to_string(),
            serde_json::Value::from(warning.threshold),
        );
        object.insert(
            "large_diff_warning_changed_lines".to_string(),
            serde_json::Value::from(warning.changed_lines),
        );
    }
}

fn collect_review_diff_payload(project_root: &Path, scope: &str) -> Option<Vec<u8>> {
    if scope == "uncommitted" {
        return collect_uncommitted_diff_payload(project_root);
    }

    if let Some(range) = scope.strip_prefix("range:") {
        return run_git(project_root, &["diff", "--no-color", range]);
    }

    if let Some(base) = scope.strip_prefix("base:") {
        let merge_base = run_git(project_root, &["merge-base", "HEAD", base])?;
        let merge_base = String::from_utf8(merge_base).ok()?;
        let merge_base = merge_base.trim();
        if merge_base.is_empty() {
            return None;
        }
        let diff_range = format!("{merge_base}...HEAD");
        return run_git(project_root, &["diff", "--no-color", &diff_range]);
    }

    if let Some(commit) = scope.strip_prefix("commit:") {
        return run_git(project_root, &["show", "--no-color", commit]);
    }

    if let Some(pathspec) = scope.strip_prefix("files:") {
        return run_git(project_root, &["diff", "--no-color", "--", pathspec]);
    }

    None
}

fn compute_uncommitted_review_diff_size(project_root: &Path) -> Option<ReviewDiffSize> {
    // Tracked working-tree changes (staged + unstaged vs HEAD). Untracked,
    // never-staged files never appear in `git diff HEAD`, so they are sized
    // separately under the hard resource caps in `crate::untracked_size` (#1818)
    // and merged in; the committed-range path (`collect_review_diff_payload`) is
    // untouched.
    let payload = run_git(project_root, &["diff", "HEAD", "--no-color"])?;
    let mut size = diff_size_from_payload(&payload);
    merge_untracked_diff_size(
        &mut size,
        crate::untracked_size::untracked_diff_size(project_root),
    );
    Some(size)
}

/// Fold the untracked working-tree contribution into a tracked diff size. File
/// counts, changed lines, and bytes add; the untracked cap/estimate/truncation
/// notes are appended so the rendered report distinguishes exact totals from
/// lower bounds. Saturating arithmetic keeps a pathological working tree from
/// overflowing the report.
fn merge_untracked_diff_size(
    size: &mut ReviewDiffSize,
    untracked: crate::untracked_size::UntrackedDiffSize,
) {
    size.files = size.files.saturating_add(untracked.files);
    size.changed_lines = size.changed_lines.saturating_add(untracked.lines);
    size.bytes = size
        .bytes
        .saturating_add(usize::try_from(untracked.bytes).unwrap_or(usize::MAX));
    size.notes.extend(untracked.notes);
}

fn collect_uncommitted_diff_payload(project_root: &Path) -> Option<Vec<u8>> {
    run_git(project_root, &["diff", "HEAD", "--no-color"])
}

fn run_git(project_root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(project_root)
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

fn diff_size_from_payload(diff: &[u8]) -> ReviewDiffSize {
    let diff_text = String::from_utf8_lossy(diff);
    let mut files = BTreeSet::new();
    let mut changed_lines = 0;
    let mut in_hunk = false;

    for line in diff_text.lines() {
        if let Some(path) = line.strip_prefix("diff --git ") {
            files.insert(path.to_string());
            in_hunk = false;
            continue;
        }
        if line.starts_with("@@") {
            in_hunk = true;
            continue;
        }
        if in_hunk && (line.starts_with('+') || line.starts_with('-')) {
            changed_lines += 1;
        }
    }

    ReviewDiffSize {
        files: files.len(),
        changed_lines,
        bytes: diff.len(),
        notes: Vec::new(),
    }
}

#[cfg(test)]
#[path = "review_cmd_diff_size_tests.rs"]
mod tests;
