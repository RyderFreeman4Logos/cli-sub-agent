use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use csa_session::state::{ReviewSessionMeta, write_review_meta};
use csa_session::{ReviewDiffSize, ReviewVerdictArtifact, write_review_verdict};
use tracing::{debug, warn};

pub(super) fn compute_review_diff_size(project_root: &Path, scope: &str) -> Option<ReviewDiffSize> {
    let diff = collect_review_diff_payload(project_root, scope)?;
    Some(diff_size_from_payload(&diff))
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

pub(super) fn persist_review_meta_with_diff_size(
    project_root: &Path,
    meta: &ReviewSessionMeta,
    diff_size: Option<&ReviewDiffSize>,
) {
    match csa_session::get_session_dir(project_root, &meta.session_id) {
        Ok(session_dir) => {
            if let Err(error) = write_review_meta_with_diff_size(&session_dir, meta, diff_size) {
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

pub(super) fn write_review_meta_with_diff_size(
    session_dir: &Path,
    meta: &ReviewSessionMeta,
    diff_size: Option<&ReviewDiffSize>,
) -> std::io::Result<()> {
    let Some(diff_size) = diff_size else {
        return write_review_meta(session_dir, meta);
    };

    let path = session_dir.join("review_meta.json");
    let mut value = serde_json::to_value(meta).map_err(std::io::Error::other)?;
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "diff_size".to_string(),
            serde_json::to_value(diff_size).map_err(std::io::Error::other)?,
        );
    }
    let json = serde_json::to_string_pretty(&value).map_err(std::io::Error::other)?;
    std::fs::write(path, json)
}

pub(super) fn persist_review_verdict_diff_size(
    project_root: &Path,
    session_id: &str,
    artifact: &mut ReviewVerdictArtifact,
    diff_size: Option<&ReviewDiffSize>,
) {
    let Some(diff_size) = diff_size else {
        return;
    };
    artifact.diff_size = Some((*diff_size).clone());
    let Ok(session_dir) = csa_session::get_session_dir(project_root, session_id) else {
        return;
    };
    if let Err(error) = write_review_verdict(&session_dir, artifact) {
        warn!(
            session_id,
            error = %error,
            "Failed to rewrite review-verdict.json with diff_size"
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
