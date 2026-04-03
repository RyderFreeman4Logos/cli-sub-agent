use super::tests::{git_status_porcelain, setup_git_repo};
use super::*;

#[test]
fn ignores_internal_tmp_prompt_file_creation() {
    let repo = setup_git_repo();
    let guard = maybe_capture_new_file_guard(repo.path())
        .expect("capture should succeed")
        .expect("git repo should return guard");

    fs::create_dir_all(repo.path().join(".tmp")).expect("create temp dir");
    fs::write(repo.path().join(".tmp/review.prompt.md"), "prompt\n").expect("write prompt file");

    let violation = guard.enforce_and_remove().expect("enforce should run");
    assert!(
        violation.is_none(),
        "internal prompt temp files should be ignored"
    );
    assert!(repo.path().join(".tmp/review.prompt.md").exists());
}

#[test]
fn ignores_internal_tmp_prompt_file_modification() {
    let repo = setup_git_repo();
    fs::create_dir_all(repo.path().join(".tmp")).expect("create temp dir");
    fs::write(repo.path().join(".tmp/review.prompt.md"), "before\n").expect("write prompt file");

    let guard = maybe_capture_new_file_guard(repo.path())
        .expect("capture should succeed")
        .expect("git repo should return guard");

    fs::write(repo.path().join(".tmp/review.prompt.md"), "after\n").expect("rewrite prompt file");

    let violation = guard.enforce_and_remove().expect("enforce should run");
    assert!(
        violation.is_none(),
        "internal prompt temp file rewrites should be ignored"
    );
    assert_eq!(
        fs::read_to_string(repo.path().join(".tmp/review.prompt.md")).expect("read prompt file"),
        "after\n"
    );
}

#[test]
fn still_blocks_other_tmp_file_creation() {
    let repo = setup_git_repo();
    let guard = maybe_capture_new_file_guard(repo.path())
        .expect("capture should succeed")
        .expect("git repo should return guard");

    fs::create_dir_all(repo.path().join(".tmp")).expect("create temp dir");
    fs::write(repo.path().join(".tmp/review-notes.md"), "notes\n").expect("write temp file");

    let violation = guard
        .enforce_and_remove()
        .expect("enforce should run")
        .expect("non-prompt temp files should still be blocked");
    assert!(
        violation
            .new_paths
            .iter()
            .any(|path| path == Path::new(".tmp/review-notes.md"))
    );
    assert!(!repo.path().join(".tmp/review-notes.md").exists());
    assert!(git_status_porcelain(repo.path()).trim().is_empty());
}
