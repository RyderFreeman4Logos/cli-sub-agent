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
fn adds_branch_protection_when_review_check_already_present() {
    let input =
        "pre-push:\n  commands:\n    review-check:\n      run: scripts/hooks/review-check.sh\n";
    let td = TempDir::new().unwrap();
    let lf = td.path().join("lefthook.yml");
    fs::write(&lf, input).unwrap();
    merge_lefthook_review_gate(td.path()).unwrap();
    let content = fs::read_to_string(&lf).unwrap();
    assert_eq!(content.matches("review-check:").count(), 1);
    assert_eq!(content.matches("branch-protection:").count(), 1);
}

#[test]
fn idempotent_when_pre_push_review_gate_already_present() {
    let input = "pre-push:\n  commands:\n    branch-protection:\n      run: scripts/hooks/branch-protection.sh\n    review-check:\n      run: scripts/hooks/review-check.sh\n";
    let td = TempDir::new().unwrap();
    let lf = td.path().join("lefthook.yml");
    fs::write(&lf, input).unwrap();
    merge_lefthook_review_gate(td.path()).unwrap();
    let content = fs::read_to_string(&lf).unwrap();
    assert_eq!(content, input);
}

#[test]
fn inserts_after_commands_in_existing_pre_push() {
    let input =
        "pre-push:\n  commands:\n    version-check:\n      run: scripts/hooks/version-check.sh\n";
    let result = build_merged_lefthook(input);
    assert!(
        result.contains("    branch-protection:"),
        "branch-protection entry inserted"
    );
    assert!(result.contains("    review-check:"), "entry inserted");
    let bp_pos = result.find("    branch-protection:").unwrap();
    let rc_pos = result.find("    review-check:").unwrap();
    let vc_pos = result.find("    version-check:").unwrap();
    assert!(bp_pos < rc_pos, "branch-protection before review-check");
    assert!(rc_pos < vc_pos, "review-check before version-check");
}

#[test]
fn appends_section_when_no_pre_push() {
    let input = "pre-commit:\n  commands:\n    quality-gates:\n      run: just pre-commit\n";
    let result = build_merged_lefthook(input);
    assert!(result.contains("pre-push:"), "pre-push section added");
    assert!(
        result.contains("branch-protection:"),
        "branch-protection entry added"
    );
    assert!(result.contains("review-check:"), "entry added");
}

#[test]
fn creates_minimal_lefthook_from_empty() {
    let result = build_merged_lefthook("");
    assert!(result.contains("pre-push:"));
    assert!(result.contains("branch-protection:"));
    assert!(result.contains("review-check:"));
}

#[test]
fn preserves_trailing_newline() {
    let input = "no_tty: true\npre-push:\n  commands:\n    x:\n      run: x\n";
    let result = build_merged_lefthook(input);
    assert!(result.ends_with('\n'));
}

#[test]
fn review_check_template_uses_safe_branch_for_agent_facing_output() {
    assert!(
        REVIEW_CHECK_TEMPLATE.contains(
            "<!-- CSA:REVIEW_GATE_BLOCKED branch=\"${SAFE_BRANCH}\" head_sha=\"${CURRENT_HEAD}\" -->"
        ),
        "blocked marker must expose sanitized branch"
    );
    assert!(
        REVIEW_CHECK_TEMPLATE
            .contains("Review gate marker found for ${SAFE_BRANCH} at ${SHORT_SHA}"),
        "marker-found line must expose sanitized branch"
    );
    assert!(
        REVIEW_CHECK_TEMPLATE
            .contains("csa review session recorded for ${SAFE_BRANCH} at ${SHORT_SHA}"),
        "final error must expose sanitized branch"
    );
    assert!(
        !REVIEW_CHECK_TEMPLATE.contains("CSA:REVIEW_GATE_BLOCKED branch=\"${CURRENT_BRANCH}\""),
        "blocked marker must not expose raw branch"
    );
    assert!(
        !REVIEW_CHECK_TEMPLATE.contains("Review gate marker found for ${CURRENT_BRANCH}"),
        "marker-found line must not expose raw branch"
    );
    assert!(
        !REVIEW_CHECK_TEMPLATE.contains("csa review session recorded for ${CURRENT_BRANCH}"),
        "final error must not expose raw branch"
    );
}

#[test]
fn review_gate_templates_keep_raw_branch_for_logic_and_align_protected_list() {
    assert!(
        REVIEW_CHECK_TEMPLATE.contains("SAFE_BRANCH=\"$(_sanitize_branch \"${CURRENT_BRANCH}\")\""),
        "review-check must derive display and marker path from raw branch"
    );
    assert!(
        REVIEW_CHECK_TEMPLATE.contains("PROTECTED=\"main dev master\""),
        "review-check protected branch list must include main/dev/master"
    );
    assert!(
        BRANCH_PROTECTION_TEMPLATE.contains("PROTECTED=\"main dev master\""),
        "branch-protection protected branch list must include main/dev/master"
    );
}

#[test]
fn generated_pre_push_wiring_invokes_branch_protection_before_review_check() {
    let result = build_merged_lefthook("");
    let branch_protection_pos = result.find("    branch-protection:").unwrap();
    let review_check_pos = result.find("    review-check:").unwrap();
    assert!(
        branch_protection_pos < review_check_pos,
        "protected branch pushes must be rejected before review-check can skip them"
    );
    assert!(result.contains("      run: scripts/hooks/branch-protection.sh"));
    assert!(result.contains("      run: scripts/hooks/review-check.sh"));
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
        !td.path()
            .join("scripts/hooks/branch-protection.sh")
            .exists(),
        "non-opted-in repo must not receive branch-protection.sh"
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
        lefthook_yml.contains("branch-protection:"),
        "opted-in repo receives pre-push branch protection"
    );
    assert!(
        td.path()
            .join("scripts/hooks/branch-protection.sh")
            .exists(),
        "opted-in repo receives branch-protection.sh"
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
