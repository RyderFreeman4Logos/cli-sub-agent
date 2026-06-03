use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use csa_config::{GlobalConfig, ProjectConfig, ReviewConfig};
use csa_session::state::{ReviewSessionMeta, write_review_meta};
use csa_session::{ReviewDiffSize, ReviewVerdictArtifact, write_review_verdict};
use tracing::{debug, warn};

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
    eprintln!("warning: {}", warning.message());
}

pub(super) fn format_review_diff_size_line(diff_size: &ReviewDiffSize) -> String {
    format!(
        "Diff size: {} files, {} changed lines, {} bytes",
        diff_size.files, diff_size.changed_lines, diff_size.bytes
    )
}

pub(super) fn add_review_diff_size_line(
    output: &str,
    diff_size: Option<&ReviewDiffSize>,
) -> String {
    let Some(diff_size) = diff_size else {
        return output.to_string();
    };
    let line = format_review_diff_size_line(diff_size);
    if output.starts_with(&line) {
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
    for file_name in ["summary.md", "details.md"] {
        let path = output_dir.join(file_name);
        let Ok(existing) = std::fs::read_to_string(&path) else {
            continue;
        };
        if existing.starts_with(&line) {
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
        let mut payload = run_git(project_root, &["diff", "--staged", "--no-color"])?;
        payload.extend(run_git(project_root, &["diff", "--no-color"])?);
        return Some(payload);
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

    for line in diff_text.lines() {
        if let Some(path) = line.strip_prefix("diff --git ") {
            files.insert(path.to_string());
            continue;
        }
        if (line.starts_with('+') && !line.starts_with("+++"))
            || (line.starts_with('-') && !line.starts_with("---"))
        {
            changed_lines += 1;
        }
    }

    ReviewDiffSize {
        files: files.len(),
        changed_lines,
        bytes: diff.len(),
    }
}
