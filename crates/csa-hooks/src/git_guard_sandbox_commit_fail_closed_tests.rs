use super::*;

#[test]
fn wrapper_blocks_repeated_sandbox_commit_for_unchanged_staged_tree() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let session_dir = temp.path().join("session");
    let wrapper = session_dir.join("bin/git");
    std::fs::create_dir_all(wrapper.parent().unwrap()).unwrap();
    write_executable(&wrapper, git_wrapper_script());

    let repo = temp.path().join("repo");
    init_worktree_repo(&repo);
    std::fs::write(repo.join("fixture.txt"), "staged change\n").unwrap();
    run_git(&repo, &["add", "fixture.txt"]);

    let hook_count = temp.path().join("hook-count");
    let hook_helper = repo.join("hook-helper.sh");
    write_executable(
        repo.join(".git/hooks/pre-commit").as_path(),
        r#"#!/bin/sh
count=0
[ ! -f "${HOOK_COUNT}" ] || count="$(cat "${HOOK_COUNT}")"
count=$((count + 1))
printf '%s\n' "${count}" > "${HOOK_COUNT}"
echo "hook cannot write /var/tmp: Read-only file system" >&2
exit 1
"#,
    );

    let run_commit = || {
        Command::new(&wrapper)
            .args(["commit", "-m", "sandbox commit"])
            .current_dir(&repo)
            .env("CSA_REAL_GIT", "/usr/bin/git")
            .env("CSA_FS_SANDBOXED", "1")
            .env("CSA_SESSION_DIR", &session_dir)
            .env("HOOK_COUNT", &hook_count)
            .env("HOOK_HELPER", &hook_helper)
            .output_with_timeout()
            .expect("run guarded commit")
    };
    let head_before = git_output(&repo, &["rev-parse", "HEAD"]).stdout;
    let staged_tree_before = git_output(&repo, &["write-tree"]).stdout;

    let first = run_commit();
    assert!(!first.status.success());
    assert_eq!(std::fs::read_to_string(&hook_count).unwrap().trim(), "1");
    assert!(
        session_dir.join(".git-guard-commit-failure").is_file(),
        "the first unchanged-tree hook failure must leave a fail-closed marker"
    );
    assert_eq!(
        git_output(&repo, &["rev-parse", "HEAD"]).stdout,
        head_before
    );
    assert_eq!(
        git_output(&repo, &["write-tree"]).stdout,
        staged_tree_before,
        "failed hook must preserve the staged tree"
    );

    let second = run_commit();
    assert!(!second.status.success());
    assert_eq!(
        std::fs::read_to_string(&hook_count).unwrap().trim(),
        "1",
        "unchanged staged tree must not rerun the known-failing hook"
    );
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(stderr.contains("filesystem sandbox"), "{stderr}");
    assert!(stderr.contains("staged tree is preserved"), "{stderr}");
    assert!(stderr.contains("outside the sandbox"), "{stderr}");

    std::fs::write(repo.join("fixture.txt"), "repaired staged change\n").unwrap();
    run_git(&repo, &["add", "fixture.txt"]);
    let third = run_commit();
    assert!(!third.status.success());
    assert_eq!(
        std::fs::read_to_string(&hook_count).unwrap().trim(),
        "2",
        "a changed staged tree must get one fresh hook attempt"
    );

    std::fs::write(
        repo.join("lefthook.yml"),
        "pre-commit:\n  commands:\n    check:\n      run: ./hook-helper.sh\n",
    )
    .unwrap();
    let after_lefthook_change = run_commit();
    assert!(!after_lefthook_change.status.success());
    assert_eq!(
        std::fs::read_to_string(&hook_count).unwrap().trim(),
        "3",
        "changing lefthook config must allow one fresh hook attempt"
    );

    write_executable(
        &hook_helper,
        r#"#!/bin/sh
count="$(cat "${HOOK_COUNT}")"
printf '%s\n' "$((count + 1))" > "${HOOK_COUNT}"
echo "helper rejects commit" >&2
exit 1
"#,
    );
    run_git(&repo, &["add", "hook-helper.sh"]);
    write_executable(
        repo.join(".git/hooks/pre-commit").as_path(),
        "#!/bin/sh\nexec \"${HOOK_HELPER}\"\n",
    );
    let with_referenced_helper = run_commit();
    assert!(!with_referenced_helper.status.success());
    assert_eq!(std::fs::read_to_string(&hook_count).unwrap().trim(), "4");

    write_executable(
        &hook_helper,
        r#"#!/bin/sh
count="$(cat "${HOOK_COUNT}")"
printf '%s\n' "$((count + 1))" > "${HOOK_COUNT}"
echo "repaired helper still rejects in sandbox" >&2
exit 1
"#,
    );
    let after_helper_change = run_commit();
    assert!(!after_helper_change.status.success());
    assert_eq!(
        std::fs::read_to_string(&hook_count).unwrap().trim(),
        "5",
        "changing a hook-referenced script must allow one fresh hook attempt"
    );

    write_executable(
        repo.join(".git/hooks/reference-transaction").as_path(),
        "#!/bin/sh\necho reference transaction rejects >&2\nexit 1\n",
    );
    let after_other_hook_change = run_commit();
    assert!(!after_other_hook_change.status.success());
    assert_eq!(
        std::fs::read_to_string(&hook_count).unwrap().trim(),
        "6",
        "changing another rejecting hook must allow one fresh hook attempt"
    );
}
