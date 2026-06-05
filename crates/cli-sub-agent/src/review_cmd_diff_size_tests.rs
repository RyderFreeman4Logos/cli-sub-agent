//! Tests for [`crate::review_cmd_diff_size`], in a sibling file so the
//! implementation module stays within the per-module token budget (#1818).

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
        review_mode: None,
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
fn diff_size_from_payload_counts_hunk_content_that_looks_like_file_headers() {
    let diff = concat!(
        "diff --git a/operators.txt b/operators.txt\n",
        "index 1111111..2222222 100644\n",
        "--- a/operators.txt\n",
        "+++ b/operators.txt\n",
        "@@ -1,3 +1,3 @@\n",
        " unchanged\n",
        "---removed operator line\n",
        "+++added operator line\n",
    );

    let size = diff_size_from_payload(diff.as_bytes());

    assert_eq!(size.files, 1);
    assert_eq!(
        size.changed_lines, 2,
        "file headers must be ignored, but hunk content beginning with ++/-- must count"
    );
    assert_eq!(size.bytes, diff.len());
}

#[test]
fn committed_range_diff_size_counts_only_committed_git_diff() {
    let repo = setup_diff_size_git_repo();
    run_git_command(repo.path(), &["branch", "base"]);
    std::fs::write(repo.path().join("tracked.txt"), "baseline\ncommitted\n")
        .expect("write committed change");
    run_git_command(repo.path(), &["add", "tracked.txt"]);
    run_git_command(repo.path(), &["commit", "-m", "change tracked file"]);
    std::fs::write(repo.path().join("new.txt"), "untracked\ncontent\n")
        .expect("write untracked file");

    let size =
        compute_review_diff_size(repo.path(), "range:base...HEAD").expect("compute diff size");

    assert_eq!(size.files, 1);
    assert_eq!(size.changed_lines, 1);
    assert!(size.bytes > 0);
    assert!(size.notes.is_empty());
}

#[test]
fn uncommitted_diff_size_counts_untracked_files() {
    let repo = setup_diff_size_git_repo();
    std::fs::write(repo.path().join("new.txt"), "one\ntwo\nthree\n").expect("write untracked file");

    let size = compute_review_diff_size(repo.path(), "uncommitted").expect("compute diff size");

    // The uncommitted path now includes untracked files (#1818): the new file
    // is one of three exact lines, with no estimated/capped note.
    assert_eq!(size.files, 1);
    assert_eq!(size.changed_lines, 3);
    assert!(size.bytes > 0);
    assert!(size.notes.is_empty());
    // Three changed lines now crosses a threshold of two — previously
    // suppressed when untracked files were ignored.
    assert!(large_diff_warning(&size, Some(2)).is_some());
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
        notes: Vec::new(),
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
        notes: Vec::new(),
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
        notes: Vec::new(),
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
