use super::*;
use crate::test_support::ENV_LOCK;
use std::io::Write as _;
use tempfile::NamedTempFile;

/// RAII guard that sets `XDG_STATE_HOME` to a temp path and restores
/// the original value on drop — even if the test panics.
struct ScopedXdgOverride {
    orig: Option<String>,
    orig_home: Option<String>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl ScopedXdgOverride {
    fn new(tmp: &tempfile::TempDir) -> Self {
        let lock = ENV_LOCK.lock().expect("env lock poisoned");
        let orig = std::env::var("XDG_STATE_HOME").ok();
        let orig_home = std::env::var("HOME").ok();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).expect("create isolated HOME for merge guard tests");
        // SAFETY: test-scoped env mutation protected by GUARD_ENV_LOCK.
        unsafe {
            std::env::set_var("XDG_STATE_HOME", tmp.path().join("state").to_str().unwrap());
            std::env::set_var("HOME", home.to_str().unwrap());
        }
        Self {
            orig,
            orig_home,
            _lock: lock,
        }
    }
}

impl Drop for ScopedXdgOverride {
    fn drop(&mut self) {
        // SAFETY: restoration of test-scoped env mutation (lock still held).
        unsafe {
            match &self.orig {
                Some(v) => std::env::set_var("XDG_STATE_HOME", v),
                None => std::env::remove_var("XDG_STATE_HOME"),
            }
            match &self.orig_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}

#[test]
fn test_ensure_guard_dir_creates_wrapper() {
    let tmp = tempfile::tempdir().unwrap();
    let _xdg = ScopedXdgOverride::new(&tmp);

    let dir = ensure_guard_dir().unwrap();
    let wrapper = dir.join("gh");
    assert!(wrapper.exists(), "gh wrapper should exist");
    assert!(
        wrapper.metadata().unwrap().permissions().mode() & 0o111 != 0,
        "gh wrapper should be executable"
    );
}

#[test]
fn test_inject_merge_guard_env_sets_path() {
    let tmp = tempfile::tempdir().unwrap();
    let _xdg = ScopedXdgOverride::new(&tmp);

    let mut env = HashMap::new();
    env.insert("PATH".to_string(), "/usr/bin:/bin".to_string());
    inject_merge_guard_env(&mut env);

    let path = env.get("PATH").unwrap();
    assert!(
        path.contains("guards"),
        "PATH should contain guard dir: {path}"
    );
    assert!(
        path.ends_with("/usr/bin:/bin"),
        "original PATH should be preserved: {path}"
    );
}

#[test]
fn test_inject_merge_guard_env_sets_real_gh() {
    let tmp = tempfile::tempdir().unwrap();
    let _xdg = ScopedXdgOverride::new(&tmp);

    let mut env = HashMap::new();
    inject_merge_guard_env(&mut env);
    // CSA_REAL_GH is set only if `gh` is installed.
    // In CI, gh may not be available, so we just check the key exists
    // when we know gh is installed.
    if which::which("gh").is_ok() {
        assert!(
            env.contains_key("CSA_REAL_GH"),
            "CSA_REAL_GH should be set when gh is installed"
        );
    }
}

#[test]
fn test_is_merge_guard_enabled_default_true() {
    assert!(is_merge_guard_enabled(None));
}

#[test]
fn test_is_merge_guard_enabled_nonexistent_file() {
    let path = Path::new("/nonexistent/hooks.toml");
    assert!(is_merge_guard_enabled(Some(path)));
}

#[test]
fn test_is_merge_guard_disabled() {
    let mut f = NamedTempFile::new().unwrap();
    writeln!(f, "[hooks]").unwrap();
    writeln!(f, "merge_guard = false").unwrap();
    f.flush().unwrap();
    assert!(!is_merge_guard_enabled(Some(f.path())));
}

#[test]
fn test_is_merge_guard_enabled_explicit() {
    let mut f = NamedTempFile::new().unwrap();
    writeln!(f, "[hooks]").unwrap();
    writeln!(f, "merge_guard = true").unwrap();
    f.flush().unwrap();
    assert!(is_merge_guard_enabled(Some(f.path())));
}

// --- verify_pr_bot_marker tests ---

/// Helper: create a marker file in the temp directory.
fn create_marker(base: &Path, repo: &str, filename: &str) {
    let dir = base.join(repo);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(filename), "").unwrap();
}

#[test]
fn test_verify_marker_exact_sha_match() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    create_marker(base, "owner_repo", "42-abc123def.done");

    assert_eq!(
        verify_pr_bot_marker(base, "owner_repo", 42, "abc123def"),
        MarkerStatus::Verified
    );
}

#[test]
fn test_verify_marker_wrong_sha_with_stale() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    // Old marker exists for a previous SHA.
    create_marker(base, "owner_repo", "42-oldsha999.done");

    assert_eq!(
        verify_pr_bot_marker(base, "owner_repo", 42, "newsha000"),
        MarkerStatus::StaleMarkerExists
    );
}

#[test]
fn test_verify_marker_missing_no_markers() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    // No marker directory at all.
    assert_eq!(
        verify_pr_bot_marker(base, "owner_repo", 42, "abc123def"),
        MarkerStatus::Missing
    );
}

#[test]
fn test_verify_marker_missing_dir_exists_but_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    fs::create_dir_all(base.join("owner_repo")).unwrap();

    assert_eq!(
        verify_pr_bot_marker(base, "owner_repo", 42, "abc123def"),
        MarkerStatus::Missing
    );
}

// --- GH wrapper auto-merge block tests ---
//
// These tests execute the wrapper script with a fake `gh` binary to verify
// that `--auto` / `--enable-auto-merge` are unconditionally blocked.

/// Create a minimal test environment with a fake `gh` and the wrapper script.
/// Returns (guard_dir, fake_gh_path).
fn setup_wrapper_env(tmp: &Path) -> (PathBuf, PathBuf) {
    let guard_dir = tmp.join("guard");
    fs::create_dir_all(&guard_dir).unwrap();

    // Write the wrapper script.
    let wrapper_path = guard_dir.join("gh");
    fs::write(&wrapper_path, GH_WRAPPER).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&wrapper_path, fs::Permissions::from_mode(0o755)).unwrap();

    // Create a fake `gh` that just prints "REAL_GH_CALLED" and exits 0.
    let fake_gh = tmp.join("fake_gh");
    fs::write(&fake_gh, "#!/bin/bash\necho REAL_GH_CALLED\n").unwrap();
    #[cfg(unix)]
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).unwrap();

    (guard_dir, fake_gh)
}

/// Run the wrapper with given args and return (exit_code, stdout, stderr).
fn run_wrapper(guard_dir: &Path, fake_gh: &Path, args: &[&str]) -> (i32, String, String) {
    let wrapper = guard_dir.join("gh");
    let output = std::process::Command::new("bash")
        .arg(&wrapper)
        .args(args)
        .env("CSA_REAL_GH", fake_gh.to_str().unwrap())
        .output()
        .expect("failed to run wrapper");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

#[test]
fn test_wrapper_blocks_auto_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let (guard_dir, fake_gh) = setup_wrapper_env(tmp.path());

    let (code, _stdout, stderr) =
        run_wrapper(&guard_dir, &fake_gh, &["pr", "merge", "123", "--auto"]);
    assert_eq!(code, 1, "should exit 1 for --auto");
    assert!(
        stderr.contains("auto-merge is prohibited"),
        "stderr should contain prohibition message: {stderr}"
    );
}

#[test]
fn test_wrapper_blocks_enable_auto_merge_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let (guard_dir, fake_gh) = setup_wrapper_env(tmp.path());

    let (code, _stdout, stderr) = run_wrapper(
        &guard_dir,
        &fake_gh,
        &["pr", "merge", "123", "--enable-auto-merge"],
    );
    assert_eq!(code, 1, "should exit 1 for --enable-auto-merge");
    assert!(
        stderr.contains("auto-merge is prohibited"),
        "stderr should contain prohibition message: {stderr}"
    );
}

#[test]
fn test_wrapper_auto_flag_not_bypassed_by_force_skip() {
    let tmp = tempfile::tempdir().unwrap();
    let (guard_dir, fake_gh) = setup_wrapper_env(tmp.path());

    let (code, _stdout, stderr) = run_wrapper(
        &guard_dir,
        &fake_gh,
        &["pr", "merge", "123", "--auto", "--force-skip-pr-bot"],
    );
    assert_eq!(code, 1, "should exit 1 even with --force-skip-pr-bot");
    assert!(
        stderr.contains("auto-merge is prohibited"),
        "--force-skip-pr-bot must NOT bypass --auto block: {stderr}"
    );
}

#[test]
fn test_wrapper_non_merge_command_passes_through() {
    let tmp = tempfile::tempdir().unwrap();
    let (guard_dir, fake_gh) = setup_wrapper_env(tmp.path());

    let (code, stdout, _stderr) = run_wrapper(&guard_dir, &fake_gh, &["pr", "view", "123"]);
    assert_eq!(code, 0, "non-merge commands should pass through");
    assert!(
        stdout.contains("REAL_GH_CALLED"),
        "should forward to real gh: {stdout}"
    );
}

#[test]
fn test_wrapper_squash_merge_not_blocked_by_auto_check() {
    let tmp = tempfile::tempdir().unwrap();
    let (guard_dir, fake_gh) = setup_wrapper_env(tmp.path());

    // --squash without --auto should NOT be blocked by the auto-merge check.
    // It will proceed to the marker check (which will fail since there's no
    // marker), but the important thing is it's NOT blocked by the auto check.
    let (_code, _stdout, stderr) =
        run_wrapper(&guard_dir, &fake_gh, &["pr", "merge", "123", "--squash"]);
    assert!(
        !stderr.contains("auto-merge is prohibited"),
        "--squash should NOT trigger auto-merge block: {stderr}"
    );
}

#[test]
fn test_wrapper_blocks_cross_repo_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let (guard_dir, fake_gh) = setup_wrapper_env(tmp.path());

    let (code, _stdout, stderr) = run_wrapper(
        &guard_dir,
        &fake_gh,
        &["pr", "merge", "123", "-R", "other/repo"],
    );
    assert_eq!(code, 1, "should exit 1 for -R flag");
    assert!(
        stderr.contains("cross-repo merge"),
        "stderr should mention cross-repo: {stderr}"
    );
}

#[test]
fn test_wrapper_blocks_repo_long_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let (guard_dir, fake_gh) = setup_wrapper_env(tmp.path());

    let (code, _stdout, stderr) = run_wrapper(
        &guard_dir,
        &fake_gh,
        &["pr", "merge", "123", "--repo", "other/repo"],
    );
    assert_eq!(code, 1, "should exit 1 for --repo flag");
    assert!(
        stderr.contains("cross-repo merge"),
        "stderr should mention cross-repo: {stderr}"
    );
}

#[test]
fn test_wrapper_rejects_url_argument() {
    let tmp = tempfile::tempdir().unwrap();
    let (guard_dir, fake_gh) = setup_wrapper_env(tmp.path());

    // URLs must be rejected (fail-closed) to prevent cross-repo bypass.
    let (code, _stdout, stderr) = run_wrapper(
        &guard_dir,
        &fake_gh,
        &[
            "pr",
            "merge",
            "https://github.com/owner/repo/pull/456",
            "--squash",
        ],
    );
    assert_eq!(code, 1, "should exit 1 for URL argument");
    assert!(
        stderr.contains("only accepts numeric PR numbers"),
        "stderr should reject non-numeric arg: {stderr}"
    );
}

#[test]
fn test_wrapper_rejects_branch_name_argument() {
    let tmp = tempfile::tempdir().unwrap();
    let (guard_dir, fake_gh) = setup_wrapper_env(tmp.path());

    // Branch names must be rejected (fail-closed).
    let (code, _stdout, stderr) = run_wrapper(
        &guard_dir,
        &fake_gh,
        &["pr", "merge", "feat/my-branch", "--squash"],
    );
    assert_eq!(code, 1, "should exit 1 for branch name argument");
    assert!(
        stderr.contains("only accepts numeric PR numbers"),
        "stderr should reject non-numeric arg: {stderr}"
    );
}

#[test]
fn test_verify_marker_different_pr_not_matched() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    // Marker for PR 99, not PR 42.
    create_marker(base, "owner_repo", "99-abc123def.done");

    assert_eq!(
        verify_pr_bot_marker(base, "owner_repo", 42, "abc123def"),
        MarkerStatus::Missing
    );
}

#[test]
fn test_verify_marker_exact_takes_precedence_over_stale() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    // Both exact and stale markers exist.
    create_marker(base, "owner_repo", "42-oldsha999.done");
    create_marker(base, "owner_repo", "42-abc123def.done");

    assert_eq!(
        verify_pr_bot_marker(base, "owner_repo", 42, "abc123def"),
        MarkerStatus::Verified
    );
}

// --- install_merge_guard overwrite protection tests ---

#[test]
fn test_install_refuses_to_overwrite_non_csa_binary() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("bin");
    fs::create_dir_all(&dir).unwrap();
    // Write a fake non-CSA `gh` binary.
    fs::write(dir.join("gh"), "#!/bin/bash\necho real gh\n").unwrap();

    let result = install_merge_guard(&dir);
    assert!(result.is_err(), "should refuse to overwrite non-CSA gh");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("not a CSA wrapper"),
        "error should mention non-CSA wrapper: {err_msg}"
    );
}

#[test]
fn test_install_allows_overwrite_of_existing_csa_wrapper() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("bin");
    fs::create_dir_all(&dir).unwrap();
    // Write an existing CSA wrapper.
    fs::write(dir.join("gh"), "#!/bin/bash\n# CSA merge guard v1\n").unwrap();

    let result = install_merge_guard(&dir);
    assert!(
        result.is_ok(),
        "should allow overwriting existing CSA wrapper"
    );
    // Verify the content was updated.
    let content = fs::read_to_string(dir.join("gh")).unwrap();
    assert!(
        content.contains("pr-bot"),
        "wrapper should be updated to latest: {content}"
    );
}

#[test]
fn test_install_creates_new_wrapper_in_empty_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("newbin");

    let result = install_merge_guard(&dir);
    assert!(result.is_ok(), "should create wrapper in new dir");
    assert!(dir.join("gh").exists(), "gh wrapper should exist");
}

// --- GH wrapper --help passthrough and flag-value PR parsing tests ---

#[test]
fn test_wrapper_help_passes_through() {
    let tmp = tempfile::tempdir().unwrap();
    let (guard_dir, fake_gh) = setup_wrapper_env(tmp.path());

    let (code, stdout, _stderr) = run_wrapper(&guard_dir, &fake_gh, &["pr", "merge", "--help"]);
    assert_eq!(code, 0, "--help should pass through");
    assert!(
        stdout.contains("REAL_GH_CALLED"),
        "--help should forward to real gh: {stdout}"
    );
}

#[test]
fn test_wrapper_short_help_passes_through() {
    let tmp = tempfile::tempdir().unwrap();
    let (guard_dir, fake_gh) = setup_wrapper_env(tmp.path());

    let (code, stdout, _stderr) = run_wrapper(&guard_dir, &fake_gh, &["pr", "merge", "-h"]);
    assert_eq!(code, 0, "-h should pass through");
    assert!(
        stdout.contains("REAL_GH_CALLED"),
        "-h should forward to real gh: {stdout}"
    );
}

#[test]
fn test_wrapper_merge_method_flag_with_value() {
    let tmp = tempfile::tempdir().unwrap();
    let (guard_dir, fake_gh) = setup_wrapper_env(tmp.path());

    // `gh pr merge --merge-method squash 123`
    // "squash" should NOT be treated as PR number; "123" should.
    let (_code, _stdout, stderr) = run_wrapper(
        &guard_dir,
        &fake_gh,
        &["pr", "merge", "--merge-method", "squash", "123"],
    );
    // Should reach the marker check (PR_NUMBER=123), not the
    // "only accepts numeric PR numbers" rejection for "squash".
    assert!(
        !stderr.contains("only accepts numeric PR numbers"),
        "--merge-method value 'squash' should not trigger rejection: {stderr}"
    );
}

#[test]
fn test_wrapper_pr_number_after_flags_with_values() {
    let tmp = tempfile::tempdir().unwrap();
    let (guard_dir, fake_gh) = setup_wrapper_env(tmp.path());

    // `gh pr merge -t "title" -b "body" 456`
    let (_code, _stdout, stderr) = run_wrapper(
        &guard_dir,
        &fake_gh,
        &["pr", "merge", "-t", "my title", "-b", "my body", "456"],
    );
    assert!(
        !stderr.contains("only accepts numeric PR numbers"),
        "flag values should not block numeric PR number: {stderr}"
    );
}

#[test]
fn test_wrapper_blocks_repo_equals_form() {
    let tmp = tempfile::tempdir().unwrap();
    let (guard_dir, fake_gh) = setup_wrapper_env(tmp.path());

    let (code, _stdout, stderr) = run_wrapper(
        &guard_dir,
        &fake_gh,
        &["pr", "merge", "123", "--repo=other/repo"],
    );
    assert_eq!(code, 1, "should exit 1 for --repo=owner/repo");
    assert!(
        stderr.contains("cross-repo merge"),
        "stderr should mention cross-repo: {stderr}"
    );
}

#[test]
fn test_wrapper_numeric_flag_value_not_treated_as_pr_number() {
    let tmp = tempfile::tempdir().unwrap();
    let (guard_dir, _) = setup_wrapper_env(tmp.path());

    // Create a fake `gh` that fails on `pr view` (no current PR) but
    // succeeds on everything else. This simulates "no PR on current branch".
    let fake_gh = tmp.path().join("fake_gh_smart");
    fs::write(
        &fake_gh,
        "#!/bin/bash\n\
         # Fail on 'pr view' to simulate no current-branch PR.\n\
         for a in \"$@\"; do [ \"$a\" = \"view\" ] && exit 1; done\n\
         echo REAL_GH_CALLED\n",
    )
    .unwrap();
    #[cfg(unix)]
    fs::set_permissions(&fake_gh, fs::Permissions::from_mode(0o755)).unwrap();

    // `gh pr merge -t 123` — 123 is the subject/title, NOT a PR number.
    // Without an actual PR number, the wrapper should fall through to
    // `gh pr view --json number` (which will fail), then emit
    // "cannot determine PR number". It must NOT treat 123 as PR number.
    let (_code, _stdout, stderr) = run_wrapper(&guard_dir, &fake_gh, &["pr", "merge", "-t", "123"]);
    assert!(
        !stderr.contains("pr-bot has not completed for PR #123"),
        "-t 123: '123' must not be treated as PR number: {stderr}"
    );
    // Should reach the "cannot determine PR number" path instead.
    assert!(
        stderr.contains("cannot determine PR number"),
        "-t 123: should fail to determine PR number: {stderr}"
    );
}

#[test]
fn test_wrapper_author_email_flag_not_blocking() {
    let tmp = tempfile::tempdir().unwrap();
    let (guard_dir, fake_gh) = setup_wrapper_env(tmp.path());

    // `gh pr merge --author-email foo@example.com 789`
    // "foo@example.com" should NOT be treated as a non-numeric positional.
    let (_code, _stdout, stderr) = run_wrapper(
        &guard_dir,
        &fake_gh,
        &["pr", "merge", "--author-email", "foo@example.com", "789"],
    );
    assert!(
        !stderr.contains("only accepts numeric PR numbers"),
        "--author-email value should not trigger rejection: {stderr}"
    );
}

#[test]
fn test_wrapper_author_email_equals_form_not_blocking() {
    let tmp = tempfile::tempdir().unwrap();
    let (guard_dir, fake_gh) = setup_wrapper_env(tmp.path());

    // `gh pr merge --author-email=foo@example.com 789`
    let (_code, _stdout, stderr) = run_wrapper(
        &guard_dir,
        &fake_gh,
        &["pr", "merge", "--author-email=foo@example.com", "789"],
    );
    assert!(
        !stderr.contains("only accepts numeric PR numbers"),
        "--author-email=value should not trigger rejection: {stderr}"
    );
}

#[test]
fn test_wrapper_author_email_short_flag_not_blocking() {
    let tmp = tempfile::tempdir().unwrap();
    let (guard_dir, fake_gh) = setup_wrapper_env(tmp.path());

    // `gh pr merge -A foo@example.com 789`
    let (_code, _stdout, stderr) = run_wrapper(
        &guard_dir,
        &fake_gh,
        &["pr", "merge", "-A", "foo@example.com", "789"],
    );
    assert!(
        !stderr.contains("only accepts numeric PR numbers"),
        "-A value should not trigger rejection: {stderr}"
    );
}
