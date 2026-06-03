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
    eprintln!("{}", format_large_diff_warning(warning));
}

pub(super) fn format_large_diff_warning(warning: LargeDiffWarning) -> String {
    format!("warning: {}", warning.message())
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

fn collect_uncommitted_diff_payload(project_root: &Path) -> Option<Vec<u8>> {
    let mut payload = run_git(project_root, &["diff", "HEAD", "--no-color"])?;
    append_untracked_file_diffs(project_root, &mut payload)?;
    Some(payload)
}

fn append_untracked_file_diffs(project_root: &Path, payload: &mut Vec<u8>) -> Option<()> {
    let paths = run_git(
        project_root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;

    for path in paths
        .split(|byte| *byte == b'\0')
        .filter(|path| !path.is_empty())
    {
        append_untracked_file_diff(project_root, path, payload)?;
    }

    Some(())
}

fn append_untracked_file_diff(
    project_root: &Path,
    relative_path: &[u8],
    payload: &mut Vec<u8>,
) -> Option<()> {
    let relative_path = String::from_utf8_lossy(relative_path);
    let content = std::fs::read(project_root.join(relative_path.as_ref())).ok()?;
    let line_count = content_line_count(&content);

    if !payload.is_empty() && !payload.ends_with(b"\n") {
        payload.push(b'\n');
    }

    payload.extend_from_slice(
        format!(
            "diff --git a/{relative_path} b/{relative_path}\nnew file mode 100644\nindex 0000000..0000000\n--- /dev/null\n+++ b/{relative_path}\n@@ -0,0 +1,{line_count} @@\n",
        )
        .as_bytes(),
    );

    for line in content.split_inclusive(|byte| *byte == b'\n') {
        payload.push(b'+');
        payload.extend_from_slice(line);
        if !line.ends_with(b"\n") {
            payload.push(b'\n');
        }
    }

    Some(())
}

fn content_line_count(content: &[u8]) -> usize {
    if content.is_empty() {
        return 0;
    }

    let newline_count = content.iter().filter(|byte| **byte == b'\n').count();
    if content.ends_with(b"\n") {
        newline_count
    } else {
        newline_count + 1
    }
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

#[cfg(test)]
mod tests {
    use std::process::Command;

    use csa_core::types::ReviewDecision;
    use csa_session::ReviewVerdictArtifact;
    use tempfile::tempdir;

    use super::*;

    fn run_git_command(project_root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(project_root)
            .output()
            .expect("git command should execute");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn setup_diff_size_git_repo() -> tempfile::TempDir {
        let temp = tempdir().expect("tempdir");
        run_git_command(temp.path(), &["init"]);
        run_git_command(temp.path(), &["config", "user.email", "test@example.com"]);
        run_git_command(temp.path(), &["config", "user.name", "Test User"]);
        std::fs::write(temp.path().join("tracked.txt"), "baseline\n").expect("write tracked file");
        run_git_command(temp.path(), &["add", "tracked.txt"]);
        run_git_command(temp.path(), &["commit", "-m", "initial"]);
        temp
    }

    fn review_meta(session_id: &str) -> ReviewSessionMeta {
        ReviewSessionMeta {
            session_id: session_id.to_string(),
            head_sha: "HEAD".to_string(),
            decision: ReviewDecision::Pass.as_str().to_string(),
            verdict: "CLEAN".to_string(),
            status_reason: None,
            routed_to: None,
            primary_failure: None,
            failure_reason: None,
            tool: "codex".to_string(),
            scope: "range:main...HEAD".to_string(),
            exit_code: 0,
            fix_attempted: false,
            fix_rounds: 0,
            review_iterations: 1,
            timestamp: chrono::Utc::now(),
            diff_fingerprint: Some("fingerprint".to_string()),
            fix_convergence: None,
        }
    }

    #[test]
    fn diff_size_from_payload_counts_multi_file_changed_lines_and_bytes() {
        let diff = concat!(
            "diff --git a/one.txt b/one.txt\n",
            "index 1111111..2222222 100644\n",
            "--- a/one.txt\n",
            "+++ b/one.txt\n",
            "@@ -1,2 +1,3 @@\n",
            " keep\n",
            "-old one\n",
            "+new one\n",
            "+added one\n",
            "diff --git a/two.txt b/two.txt\n",
            "index 3333333..4444444 100644\n",
            "--- a/two.txt\n",
            "+++ b/two.txt\n",
            "@@ -1 +1 @@\n",
            "-old two\n",
            "+new two\n",
        );

        let size = diff_size_from_payload(diff.as_bytes());

        assert_eq!(size.files, 2);
        assert_eq!(size.changed_lines, 5);
        assert_eq!(size.bytes, diff.len());
    }

    #[test]
    fn uncommitted_diff_size_counts_untracked_files_and_large_diff_warning() {
        let repo = setup_diff_size_git_repo();
        std::fs::write(repo.path().join("new.txt"), "one\ntwo\nthree\n")
            .expect("write untracked file");

        let size = compute_review_diff_size(repo.path(), "uncommitted").expect("compute diff size");

        assert!(size.files >= 1);
        assert!(size.changed_lines > 0);
        assert_eq!(size.changed_lines, 3);
        let warning = large_diff_warning(&size, Some(2)).expect("untracked additions warn");
        assert_eq!(warning.changed_lines, 3);
        assert_eq!(warning.threshold, 2);
    }

    #[test]
    fn uncommitted_diff_size_counts_overlapping_staged_and_unstaged_edit_once() {
        let repo = setup_diff_size_git_repo();
        let tracked_path = repo.path().join("tracked.txt");
        std::fs::write(&tracked_path, "staged\n").expect("write staged version");
        run_git_command(repo.path(), &["add", "tracked.txt"]);
        std::fs::write(&tracked_path, "final\n").expect("write unstaged version");

        let size = compute_review_diff_size(repo.path(), "uncommitted").expect("compute diff size");

        assert_eq!(size.files, 1);
        assert_eq!(size.changed_lines, 2);
    }

    #[test]
    fn large_diff_warning_respects_threshold_boundaries_and_disabled_values() {
        let size = ReviewDiffSize {
            files: 3,
            changed_lines: 1001,
            bytes: 4096,
        };

        assert!(large_diff_warning(&size, Some(1001)).is_none());
        assert!(large_diff_warning(&size, Some(0)).is_none());
        assert!(large_diff_warning(&size, None).is_none());

        let warning = large_diff_warning(&size, Some(1000)).expect("above threshold warns");
        assert_eq!(warning.changed_lines, 1001);
        assert_eq!(warning.threshold, 1000);
        assert!(format_large_diff_warning(warning).starts_with("warning: review diff is large"));
    }

    #[test]
    fn write_review_meta_with_diff_report_records_size_and_large_diff_warning() {
        let session_dir = tempdir().expect("tempdir");
        let diff_size = ReviewDiffSize {
            files: 2,
            changed_lines: 1549,
            bytes: 8192,
        };
        let warning = LargeDiffWarning {
            changed_lines: 1549,
            threshold: 1000,
        };

        write_review_meta_with_diff_report(
            session_dir.path(),
            &review_meta("01REVIEWMETA000000000000"),
            Some(&diff_size),
            Some(warning),
        )
        .expect("write review meta");

        let raw = std::fs::read_to_string(session_dir.path().join("review_meta.json"))
            .expect("read review meta");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("parse review meta");
        assert_eq!(value["diff_size"]["files"], 2);
        assert_eq!(value["diff_size"]["changed_lines"], 1549);
        assert_eq!(value["diff_size"]["bytes"], 8192);
        assert_eq!(value["large_diff_warning"], true);
        assert_eq!(value["large_diff_warning_threshold"], 1000);
        assert_eq!(value["large_diff_warning_changed_lines"], 1549);
    }

    #[test]
    fn persist_review_verdict_diff_report_records_size_without_changing_pass_exit_code() {
        let mut artifact = ReviewVerdictArtifact::from_parts(
            "01VERDICT00000000000000",
            ReviewDecision::Pass,
            "CLEAN",
            &[],
            Vec::new(),
        );
        let diff_size = ReviewDiffSize {
            files: 2,
            changed_lines: 1549,
            bytes: 8192,
        };
        let warning = LargeDiffWarning {
            changed_lines: 1549,
            threshold: 1000,
        };
        let project_root = tempdir().expect("tempdir");

        persist_review_verdict_diff_report(
            project_root.path(),
            "01VERDICT00000000000000",
            &mut artifact,
            Some(&diff_size),
            Some(warning),
        );

        assert_eq!(artifact.decision, ReviewDecision::Pass);
        assert_eq!(artifact.verdict_legacy, "CLEAN");
        assert_eq!(
            crate::verdict_exit_code::exit_code_from_review_decision(artifact.decision),
            0
        );
        assert_eq!(artifact.diff_size, Some(diff_size));
        assert!(artifact.large_diff_warning);
        assert_eq!(artifact.large_diff_warning_threshold, Some(1000));
        assert_eq!(artifact.large_diff_warning_changed_lines, Some(1549));
    }

    #[test]
    fn resolve_large_diff_warn_lines_uses_default_when_absent_and_project_override_when_set() {
        let global: GlobalConfig = toml::from_str("[review]\ntool = \"auto\"\n")
            .expect("parse global config without threshold");
        assert_eq!(resolve_large_diff_warn_lines(None, &global), Some(1000));

        let project: ProjectConfig =
            toml::from_str("schema_version = 1\n[review]\nlarge_diff_warn_lines = 0\n")
                .expect("parse project config with disabled threshold");
        assert_eq!(
            resolve_large_diff_warn_lines(Some(&project), &GlobalConfig::default()),
            Some(0)
        );
    }
}
