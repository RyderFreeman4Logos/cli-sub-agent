use super::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;

fn run_quiet(command: &mut Command) {
    let status = command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
}

fn init_review_check_repo() -> TempDir {
    let td = TempDir::new().unwrap();

    run_quiet(Command::new("git").args(["init"]).current_dir(td.path()));

    for args in [
        ["config", "user.email", "test@example.com"],
        ["config", "user.name", "Test User"],
    ] {
        run_quiet(Command::new("git").args(args).current_dir(td.path()));
    }

    fs::write(td.path().join("tracked.txt"), "content\n").unwrap();
    run_quiet(
        Command::new("git")
            .args(["add", "tracked.txt"])
            .current_dir(td.path()),
    );
    run_quiet(
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(td.path()),
    );
    run_quiet(
        Command::new("git")
            .args(["checkout", "-b", "feature/review-check-test"])
            .current_dir(td.path()),
    );

    install_review_check_script(td.path()).unwrap();
    td
}

fn install_fake_csa(project_root: &Path) -> PathBuf {
    let bin_dir = project_root.join("fake-bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let csa_path = bin_dir.join("csa");
    fs::write(
        &csa_path,
        r#"#!/usr/bin/env bash
printf 'called\n' > "${CSA_FAKE_CALLED}"
exit 2
"#,
    )
    .unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&csa_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&csa_path, perms).unwrap();
    }

    bin_dir
}

fn run_review_check(
    project_root: &Path,
    fake_bin: &Path,
    fake_called_path: &Path,
    csa_session_id: Option<&str>,
    csa_depth: Option<&str>,
) -> std::process::Output {
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut command = Command::new("bash");
    command
        .arg("scripts/hooks/review-check.sh")
        .current_dir(project_root)
        .env("PATH", path)
        .env("CSA_FAKE_CALLED", fake_called_path)
        .env_remove("CSA_SKIP_REVIEW_CHECK")
        .env_remove("CSA_SESSION_ID")
        .env_remove("CSA_DEPTH");

    if let Some(value) = csa_session_id {
        command.env("CSA_SESSION_ID", value);
    }
    if let Some(value) = csa_depth {
        command.env("CSA_DEPTH", value);
    }

    command.output().unwrap()
}

#[test]
fn review_check_skips_inside_csa_session_id_executor() {
    let td = init_review_check_repo();
    let fake_bin = install_fake_csa(td.path());
    let fake_called = td.path().join("csa-called");

    let output = run_review_check(
        td.path(),
        &fake_bin,
        &fake_called,
        Some("01TESTSESSION000000000000"),
        None,
    );

    assert!(output.status.success());
    assert!(!fake_called.exists(), "review-check must not invoke csa");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Review gate skipped inside CSA executor session"));
}

#[test]
fn review_check_skips_inside_nested_csa_depth() {
    let td = init_review_check_repo();
    let fake_bin = install_fake_csa(td.path());
    let fake_called = td.path().join("csa-called");

    let output = run_review_check(td.path(), &fake_bin, &fake_called, None, Some("1"));

    assert!(output.status.success());
    assert!(!fake_called.exists(), "review-check must not invoke csa");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Review gate skipped inside CSA executor session"));
}

#[test]
fn review_check_still_invokes_csa_outside_executor() {
    let td = init_review_check_repo();
    let fake_bin = install_fake_csa(td.path());
    let fake_called = td.path().join("csa-called");

    let output = run_review_check(td.path(), &fake_bin, &fake_called, None, None);

    assert!(!output.status.success());
    assert!(
        fake_called.exists(),
        "manual pre-push path must still invoke csa"
    );
}
