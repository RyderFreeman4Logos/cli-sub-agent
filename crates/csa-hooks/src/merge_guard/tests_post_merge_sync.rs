//! Post-merge sync tests for the `gh` wrapper script.
//!
//! Split from `tests.rs` to stay under monolith-file limits.

use super::*;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Setup for post-merge sync tests. Returns (guard_dir, fake_gh, mock_bin_dir, git_log_file).
fn setup_post_merge_env(
    tmp: &Path,
    gh_exit: i32,
    branch: Option<&str>,
    git_merge_exit: i32,
) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    setup_post_merge_env_with_git(tmp, gh_exit, branch, None, branch, git_merge_exit)
}

fn setup_post_merge_env_with_git(
    tmp: &Path,
    gh_exit: i32,
    branch: Option<&str>,
    origin_head: Option<&str>,
    current_branch: Option<&str>,
    git_merge_exit: i32,
) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let guard_dir = tmp.join("guard");
    fs::create_dir_all(&guard_dir).unwrap();
    let wrapper_path = guard_dir.join("gh");
    fs::write(&wrapper_path, GH_WRAPPER).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&wrapper_path, fs::Permissions::from_mode(0o755)).unwrap();

    let branch_out = match branch {
        Some(b) => format!("printf '{}\\n'", b),
        None => "exit 1".to_string(),
    };
    let origin_head_out = match origin_head {
        Some(b) => format!("printf '{}\\n'", b),
        None => "exit 1".to_string(),
    };
    let current_branch_out = match current_branch {
        Some(b) => format!("printf '{}\\n'", b),
        None => "exit 1".to_string(),
    };
    let fake_gh = tmp.join("fake_gh");
    fs::write(&fake_gh, format!(r#"#!/bin/bash
_q() {{ local N=false; for v in "$@"; do $N && echo "$v" && return; [ "$v" = "-q" ] && N=true; done; return 1; }}
for a in "$@"; do
  if [ "$a" = "view" ]; then
    Q="$(_q "$@")" || Q=""
    if [ "$1" = "repo" ]; then
      case "$Q" in *.nameWithOwner*) echo owner/repo;; *.defaultBranchRef*) {branch_out};; *) echo owner/repo;; esac; exit $?
    fi
    case "$Q" in *.headRefOid*) echo abc123;; *.number*) echo 42;; *) printf '{{"number":42,"headRefOid":"abc123"}}\n';; esac; exit 0
  fi
  [ "$a" = "merge" ] && exit {gh_exit}
done
echo REAL_GH_CALLED
"#)).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).unwrap();

    let mock_bin = tmp.join("mock_bin");
    fs::create_dir_all(&mock_bin).unwrap();
    let git_log = tmp.join("git_calls.log");
    fs::write(
        mock_bin.join("git"),
        format!(
            r#"#!/bin/bash
echo "$@" >> "{log}"
if [ "$1" = "rev-parse" ] && [ "$2" = "--abbrev-ref" ]; then
  case "$3" in
    origin/HEAD) {origin_head_out}; exit $?;;
    HEAD) {current_branch_out}; exit $?;;
  esac
fi
for a in "$@"; do [ "$a" = "--ff-only" ] && exit {me}; done; exit 0
"#,
            log = git_log.to_str().unwrap(),
            origin_head_out = origin_head_out,
            current_branch_out = current_branch_out,
            me = git_merge_exit
        ),
    )
    .unwrap();
    #[cfg(unix)]
    fs::set_permissions(&mock_bin.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
    (guard_dir, fake_gh, mock_bin, git_log)
}

fn run_wrapper_with_mock_git(
    guard_dir: &Path,
    fake_gh: &Path,
    mock_bin: &Path,
    args: &[&str],
) -> (i32, String, String) {
    let path = format!(
        "{}:{}",
        mock_bin.to_str().unwrap(),
        std::env::var("PATH").unwrap_or_default()
    );
    let o = std::process::Command::new("bash")
        .arg(guard_dir.join("gh"))
        .args(args)
        .env("CSA_REAL_GH", fake_gh)
        .env("PATH", &path)
        .env("HOME", guard_dir.parent().unwrap())
        .output()
        .expect("failed to run wrapper");
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).into(),
        String::from_utf8_lossy(&o.stderr).into(),
    )
}

fn create_test_marker(tmp: &Path) {
    let d = tmp.join(".local/state/cli-sub-agent/pr-bot-markers/owner_repo");
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("42-abc123.done"), "").unwrap();
}

fn git_log_lines(git_log: &Path) -> Vec<String> {
    let log = fs::read_to_string(git_log).unwrap_or_default();
    log.lines().map(String::from).collect()
}

fn assert_log_has_line(lines: &[String], expected: &str) {
    assert!(
        lines.iter().any(|line| line == expected),
        "missing `{expected}` in {lines:?}"
    );
}

fn assert_log_lacks_prefix(lines: &[String], prefix: &str) {
    assert!(
        !lines.iter().any(|line| line.starts_with(prefix)),
        "unexpected `{prefix}` call in {lines:?}"
    );
}

fn assert_in_place_sync_log(git_log: &Path, branch: &str) {
    let lines = git_log_lines(git_log);
    assert_log_has_line(&lines, &format!("fetch origin {branch}"));
    assert_log_has_line(&lines, &format!("merge origin/{branch} --ff-only"));
    assert_log_lacks_prefix(&lines, "checkout ");
    assert!(
        !lines
            .iter()
            .any(|line| line == &format!("fetch origin {branch}:{branch}")),
        "must not use refspec on default branch: {lines:?}"
    );
}

fn assert_refspec_sync_log(git_log: &Path, branch: &str) {
    let lines = git_log_lines(git_log);
    assert_log_has_line(&lines, &format!("fetch origin {branch}:{branch}"));
    assert_log_lacks_prefix(&lines, "checkout ");
    assert_log_lacks_prefix(&lines, "merge ");
}

#[test]
fn post_merge_sync_runs_on_success() {
    let tmp = tempfile::tempdir().unwrap();
    let (gd, gh, mb, gl) = setup_post_merge_env(tmp.path(), 0, Some("main"), 0);
    create_test_marker(tmp.path());
    let (code, _, _) = run_wrapper_with_mock_git(&gd, &gh, &mb, &["pr", "merge", "42"]);
    assert_eq!(code, 0);
    assert_in_place_sync_log(&gl, "main");
}

#[test]
fn post_merge_sync_skipped_on_merge_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let (gd, gh, mb, gl) = setup_post_merge_env(tmp.path(), 1, Some("main"), 0);
    create_test_marker(tmp.path());
    let (code, _, _) = run_wrapper_with_mock_git(&gd, &gh, &mb, &["pr", "merge", "42"]);
    assert_eq!(code, 1, "should propagate merge failure");
    assert!(
        fs::read_to_string(&gl).unwrap_or_default().is_empty(),
        "git must not be called"
    );
}

#[test]
fn post_merge_sync_failure_does_not_change_exit_code() {
    let tmp = tempfile::tempdir().unwrap();
    let (gd, gh, mb, _) = setup_post_merge_env(tmp.path(), 0, Some("main"), 1);
    create_test_marker(tmp.path());
    let (code, _, stderr) = run_wrapper_with_mock_git(&gd, &gh, &mb, &["pr", "merge", "42"]);
    assert_eq!(code, 0, "sync failure must not change exit code");
    assert!(stderr.contains("NOTE:"), "should emit NOTE: {stderr}");
}

#[test]
fn post_merge_sync_uses_default_branch_from_gh() {
    let tmp = tempfile::tempdir().unwrap();
    let (gd, gh, mb, gl) = setup_post_merge_env(tmp.path(), 0, Some("develop"), 0);
    create_test_marker(tmp.path());
    let (code, _, _) = run_wrapper_with_mock_git(&gd, &gh, &mb, &["pr", "merge", "42"]);
    assert_eq!(code, 0);
    assert_in_place_sync_log(&gl, "develop");
}

#[test]
fn post_merge_sync_uses_git_head_fallback_when_gh_view_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let (gd, gh, mb, gl) = setup_post_merge_env_with_git(
        tmp.path(),
        0,
        None,
        Some("origin/develop"),
        Some("develop"),
        0,
    );
    create_test_marker(tmp.path());
    let (code, _, _) = run_wrapper_with_mock_git(&gd, &gh, &mb, &["pr", "merge", "42"]);
    assert_eq!(code, 0);
    assert_in_place_sync_log(&gl, "develop");
}

#[test]
fn post_merge_sync_skips_when_all_default_detection_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let (gd, gh, mb, gl) =
        setup_post_merge_env_with_git(tmp.path(), 0, None, None, Some("main"), 0);
    create_test_marker(tmp.path());
    let (code, _, stderr) = run_wrapper_with_mock_git(&gd, &gh, &mb, &["pr", "merge", "42"]);
    assert_eq!(code, 0);
    assert!(
        stderr.contains("could not determine default branch"),
        "should explain skipped sync: {stderr}"
    );
    let lines = git_log_lines(&gl);
    assert_log_lacks_prefix(&lines, "fetch ");
    assert_log_lacks_prefix(&lines, "checkout ");
    assert_log_lacks_prefix(&lines, "merge ");
}

#[test]
fn post_merge_sync_uses_refspec_when_not_on_default() {
    let tmp = tempfile::tempdir().unwrap();
    let (gd, gh, mb, gl) =
        setup_post_merge_env_with_git(tmp.path(), 0, Some("main"), None, Some("feat/xyz"), 0);
    create_test_marker(tmp.path());
    let (code, _, _) = run_wrapper_with_mock_git(&gd, &gh, &mb, &["pr", "merge", "42"]);
    assert_eq!(code, 0);
    assert_refspec_sync_log(&gl, "main");
}

#[test]
fn post_merge_sync_fastforwards_in_place_when_on_default() {
    let tmp = tempfile::tempdir().unwrap();
    let (gd, gh, mb, gl) =
        setup_post_merge_env_with_git(tmp.path(), 0, Some("main"), None, Some("main"), 0);
    create_test_marker(tmp.path());
    let (code, _, _) = run_wrapper_with_mock_git(&gd, &gh, &mb, &["pr", "merge", "42"]);
    assert_eq!(code, 0);
    assert_in_place_sync_log(&gl, "main");
}

#[test]
fn force_skip_pr_bot_still_triggers_post_merge_sync() {
    let tmp = tempfile::tempdir().unwrap();
    let (gd, gh, mb, gl) = setup_post_merge_env(tmp.path(), 0, Some("main"), 0);
    let (code, _, _) =
        run_wrapper_with_mock_git(&gd, &gh, &mb, &["pr", "merge", "42", "--force-skip-pr-bot"]);
    assert_eq!(code, 0);
    assert_in_place_sync_log(&gl, "main");
}

#[test]
fn post_merge_sync_stdout_stays_clean_during_success() {
    let tmp = tempfile::tempdir().unwrap();
    let (gd, gh, mb, gl) = setup_post_merge_env(tmp.path(), 0, Some("main"), 0);
    create_test_marker(tmp.path());
    let (code, stdout, _stderr) = run_wrapper_with_mock_git(&gd, &gh, &mb, &["pr", "merge", "42"]);
    assert_eq!(code, 0);
    // Sync ran (git was called).
    assert_in_place_sync_log(&gl, "main");
    // The wrapper's stdout must NOT contain any git fetch/checkout/merge output.
    // All sync output should be on stderr only.
    assert!(
        !stdout.contains("fetch"),
        "stdout must not contain git fetch output: {stdout}"
    );
    assert!(
        !stdout.contains("checkout"),
        "stdout must not contain git checkout output: {stdout}"
    );
    assert!(
        !stdout.contains("merge"),
        "stdout must not contain git merge output: {stdout}"
    );
}
