use super::super::super::resolve::derive_scope_for_project;
use super::super::*;

fn setup_git_repo_on_main() -> tempfile::TempDir {
    let temp = setup_git_repo();
    run_git(temp.path(), &["branch", "-M", "main"]);
    temp
}

fn diff_review_args() -> ReviewArgs {
    ReviewArgs {
        diff: true,
        ..default_review_args()
    }
}

#[test]
fn derive_scope_for_project_falls_back_to_branch_diff_for_clean_feature_branch() {
    let temp = setup_git_repo_on_main();
    run_git(temp.path(), &["checkout", "-b", "fix/review-diff"]);
    std::fs::write(temp.path().join("feature.txt"), "feature\n").expect("write feature file");
    run_git(temp.path(), &["add", "feature.txt"]);
    run_git(temp.path(), &["commit", "-m", "feature change"]);

    assert_eq!(
        derive_scope_for_project(&diff_review_args(), temp.path()),
        "base:main"
    );
}

#[test]
fn derive_scope_for_project_keeps_uncommitted_scope_when_worktree_has_changes() {
    let temp = setup_git_repo_on_main();
    run_git(temp.path(), &["checkout", "-b", "fix/review-diff"]);
    std::fs::write(temp.path().join("tracked.txt"), "baseline\npending\n")
        .expect("write pending tracked change");

    assert_eq!(
        derive_scope_for_project(&diff_review_args(), temp.path()),
        "uncommitted"
    );
}

#[test]
fn derive_scope_for_project_keeps_uncommitted_scope_when_untracked_files_exist() {
    let temp = setup_git_repo_on_main();
    run_git(temp.path(), &["checkout", "-b", "fix/review-diff"]);
    std::fs::write(temp.path().join("feature.txt"), "feature\n").expect("write feature file");
    run_git(temp.path(), &["add", "feature.txt"]);
    run_git(temp.path(), &["commit", "-m", "feature change"]);
    std::fs::write(temp.path().join("untracked.txt"), "pending\n").expect("write untracked file");

    assert_eq!(
        derive_scope_for_project(&diff_review_args(), temp.path()),
        "uncommitted"
    );
}

#[test]
fn derive_scope_for_project_keeps_uncommitted_scope_on_clean_main() {
    let temp = setup_git_repo_on_main();

    assert_eq!(
        derive_scope_for_project(&diff_review_args(), temp.path()),
        "uncommitted"
    );
}
