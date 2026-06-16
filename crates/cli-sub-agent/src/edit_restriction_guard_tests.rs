use super::*;

use std::sync::OnceLock;
use tempfile::TempDir;

fn git_binary() -> &'static Path {
    static GIT_BINARY: OnceLock<PathBuf> = OnceLock::new();
    GIT_BINARY.get_or_init(|| which::which("git").unwrap_or_else(|_| PathBuf::from("git")))
}

fn run_git(repo: &Path, args: &[&str]) {
    let output = Command::new(git_binary())
        .current_dir("/")
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

pub(super) fn setup_git_repo() -> TempDir {
    let temp = TempDir::new().expect("create tempdir");
    run_git(temp.path(), &["init"]);
    run_git(temp.path(), &["config", "user.email", "test@example.com"]);
    run_git(temp.path(), &["config", "user.name", "Test User"]);

    fs::write(temp.path().join("tracked.txt"), "baseline\n").expect("write tracked file");
    run_git(temp.path(), &["add", "tracked.txt"]);
    run_git(temp.path(), &["commit", "-m", "initial"]);

    temp
}

pub(super) fn git_status_porcelain(repo: &Path) -> String {
    let output = Command::new("git")
        .current_dir("/")
        .arg("-C")
        .arg(repo)
        .args(["status", "--porcelain"])
        .output()
        .expect("git status should run");
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout).to_string()
}

#[test]
fn returns_none_for_non_git_directory() {
    let temp = TempDir::new().expect("create tempdir");
    let guard = maybe_capture_tracked_file_guard(temp.path()).expect("capture should succeed");
    assert!(guard.is_none());
}

#[test]
fn allows_new_untracked_file_creation() {
    let repo = setup_git_repo();
    let guard = maybe_capture_tracked_file_guard(repo.path())
        .expect("capture should succeed")
        .expect("git repo should return guard");

    fs::write(repo.path().join("new.md"), "new file\n").expect("write untracked file");

    let violation = guard.enforce_and_restore().expect("enforce should run");
    assert!(violation.is_none(), "new untracked files should be allowed");
    assert!(repo.path().join("new.md").exists());
}

#[test]
fn restores_newly_modified_tracked_file() {
    let repo = setup_git_repo();
    let guard = maybe_capture_tracked_file_guard(repo.path())
        .expect("capture should succeed")
        .expect("git repo should return guard");

    fs::write(repo.path().join("tracked.txt"), "tool mutation\n").expect("mutate tracked file");

    let violation = guard
        .enforce_and_restore()
        .expect("enforce should succeed")
        .expect("should detect violation");

    assert_eq!(
        fs::read_to_string(repo.path().join("tracked.txt")).expect("read restored file"),
        "baseline\n"
    );
    assert!(
        violation
            .modified_paths
            .iter()
            .any(|path| path == Path::new("tracked.txt"))
    );
    assert!(git_status_porcelain(repo.path()).trim().is_empty());
}

#[test]
fn restores_dirty_file_to_pre_run_snapshot() {
    let repo = setup_git_repo();

    fs::write(repo.path().join("tracked.txt"), "pre-existing dirty\n")
        .expect("create dirty baseline");

    let guard = maybe_capture_tracked_file_guard(repo.path())
        .expect("capture should succeed")
        .expect("git repo should return guard");

    fs::write(repo.path().join("tracked.txt"), "tool mutation\n").expect("mutate dirty file");

    let violation = guard
        .enforce_and_restore()
        .expect("enforce should succeed")
        .expect("should detect violation");

    assert_eq!(
        fs::read_to_string(repo.path().join("tracked.txt")).expect("read restored file"),
        "pre-existing dirty\n"
    );
    assert!(
        violation
            .modified_paths
            .iter()
            .any(|path| path == Path::new("tracked.txt"))
    );

    let status = git_status_porcelain(repo.path());
    assert!(status.contains(" M tracked.txt"));
}

#[test]
fn restores_staged_mutation_on_clean_file() {
    let repo = setup_git_repo();
    let guard = maybe_capture_tracked_file_guard(repo.path())
        .expect("capture should succeed")
        .expect("git repo should return guard");

    fs::write(repo.path().join("tracked.txt"), "tool mutation\n").expect("mutate tracked file");
    run_git(repo.path(), &["add", "tracked.txt"]);

    let violation = guard
        .enforce_and_restore()
        .expect("enforce should succeed")
        .expect("should detect violation");

    assert!(
        violation
            .restored_paths
            .iter()
            .any(|path| path == Path::new("tracked.txt"))
    );
    assert!(git_status_porcelain(repo.path()).trim().is_empty());
}
