use super::*;
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn run_git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git command should execute");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_git_repo(project_root: &Path) {
    run_git(project_root, &["init"]);
    run_git(project_root, &["config", "user.email", "test@example.com"]);
    run_git(project_root, &["config", "user.name", "Test User"]);
    fs::write(project_root.join("tracked.txt"), "baseline\n").expect("write baseline");
    run_git(project_root, &["add", "tracked.txt"]);
    run_git(project_root, &["commit", "-m", "initial"]);
}

fn track_file(project_root: &Path, relative_path: &str, content: &str) {
    let path = project_root.join(relative_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent directory");
    }
    fs::write(path, content).expect("write tracked file");
    run_git(project_root, &["add", relative_path]);
    run_git(project_root, &["commit", "-m", "track review gate opt-in"]);
}

#[cfg(unix)]
fn install_fake_lefthook(bin_dir: &Path, log_path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    fs::create_dir_all(bin_dir).expect("create fake bin dir");
    let fake = bin_dir.join("lefthook");
    fs::write(
        &fake,
        format!(
            "#!/bin/sh\n\
printf '%s\\n' \"$PWD $*\" >> '{}'\n\
mkdir -p .git/hooks\n\
cat > .git/hooks/pre-push <<'HOOK'\n\
#!/bin/sh\n\
# lefthook test stub\n\
HOOK\n",
            log_path.display()
        ),
    )
    .expect("write fake lefthook");
    let mut perms = fs::metadata(&fake)
        .expect("fake lefthook metadata")
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(fake, perms).expect("chmod fake lefthook");
}

#[test]
fn idempotent_when_review_check_already_present() {
    let input =
        "pre-push:\n  commands:\n    review-check:\n      run: scripts/hooks/review-check.sh\n";
    let td = TempDir::new().unwrap();
    let lf = td.path().join("lefthook.yml");
    fs::write(&lf, input).unwrap();
    merge_lefthook_review_check(td.path()).unwrap();
    let content = fs::read_to_string(&lf).unwrap();
    assert_eq!(content.matches("review-check:").count(), 1);
}

#[test]
fn inserts_after_commands_in_existing_pre_push() {
    let input =
        "pre-push:\n  commands:\n    version-check:\n      run: scripts/hooks/version-check.sh\n";
    let result = build_merged_lefthook(input);
    assert!(result.contains("    review-check:"), "entry inserted");
    let rc_pos = result.find("    review-check:").unwrap();
    let vc_pos = result.find("    version-check:").unwrap();
    assert!(rc_pos < vc_pos, "review-check before version-check");
}

#[test]
fn appends_section_when_no_pre_push() {
    let input = "pre-commit:\n  commands:\n    quality-gates:\n      run: just pre-commit\n";
    let result = build_merged_lefthook(input);
    assert!(result.contains("pre-push:"), "pre-push section added");
    assert!(result.contains("review-check:"), "entry added");
}

#[test]
fn creates_minimal_lefthook_from_empty() {
    let result = build_merged_lefthook("");
    assert!(result.contains("pre-push:"));
    assert!(result.contains("review-check:"));
}

#[test]
fn preserves_trailing_newline() {
    let input = "no_tty: true\npre-push:\n  commands:\n    x:\n      run: x\n";
    let result = build_merged_lefthook(input);
    assert!(result.ends_with('\n'));
}

#[test]
fn needs_check_true_when_no_file() {
    let td = TempDir::new().unwrap();
    let ts = td.path().join(REVIEW_GATE_TIMESTAMP_FILE);
    assert!(needs_review_gate_check(&ts).unwrap());
}

#[test]
fn needs_check_false_after_recent_write() {
    let td = TempDir::new().unwrap();
    let ts = td.path().join(REVIEW_GATE_TIMESTAMP_FILE);
    fs::write(&ts, b"").unwrap();
    assert!(!needs_review_gate_check(&ts).unwrap());
}

#[tokio::test]
async fn auto_setup_skips_repo_without_review_gate_opt_in() {
    let _env_lock = crate::test_env_lock::TEST_ENV_LOCK.lock().await;
    let td = TempDir::new().expect("create tempdir");
    init_git_repo(td.path());

    check_and_setup_review_gate_bg(td.path())
        .await
        .expect("skip should not fail");

    assert!(
        !td.path().join("lefthook.yml").exists(),
        "non-opted-in repo must not receive lefthook.yml"
    );
    assert!(
        !td.path().join("scripts/hooks/review-check.sh").exists(),
        "non-opted-in repo must not receive review-check.sh"
    );
    assert!(
        !td.path().join(".git/hooks/pre-push").exists(),
        "non-opted-in repo must not receive installed pre-push hook"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn auto_setup_installs_when_lefthook_yml_is_tracked() {
    let _env_lock = crate::test_env_lock::TEST_ENV_LOCK.lock().await;
    let td = TempDir::new().expect("create tempdir");
    init_git_repo(td.path());
    track_file(td.path(), "lefthook.yml", "pre-commit:\n");

    let bin_dir = td.path().join("bin");
    let log_path = td.path().join("lefthook.log");
    install_fake_lefthook(&bin_dir, &log_path);
    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = crate::test_env_lock::ScopedEnvVarRestore::set("PATH", &patched_path);

    check_and_setup_review_gate_bg(td.path())
        .await
        .expect("opted-in repo should install review gate");

    let lefthook_yml =
        fs::read_to_string(td.path().join("lefthook.yml")).expect("read updated lefthook.yml");
    assert!(
        lefthook_yml.contains("review-check:"),
        "opted-in repo keeps existing merge behavior"
    );
    assert!(
        td.path().join("scripts/hooks/review-check.sh").exists(),
        "opted-in repo receives review-check.sh"
    );
    assert!(
        td.path().join(".git/hooks/pre-push").exists(),
        "opted-in repo receives installed pre-push hook"
    );
    let lefthook_log = fs::read_to_string(log_path).expect("read fake lefthook log");
    assert!(
        lefthook_log.contains("install"),
        "opted-in repo should invoke lefthook install"
    );
}
