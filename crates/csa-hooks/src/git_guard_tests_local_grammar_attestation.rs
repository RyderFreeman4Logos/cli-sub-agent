#[cfg(unix)]
#[test]
fn wrapper_allows_exact_ls_remote_attestation_with_pinned_upload_pack() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("git");
    write_executable(&wrapper, git_wrapper_script());

    let repo = temp.path().join("repo");
    init_worktree_repo(&repo);
    let remote = temp.path().join("remote.git");
    init_bare_repo(temp.path(), &remote);
    run_git(
        &repo,
        &[
            "push",
            remote.to_str().expect("UTF-8 remote path"),
            "HEAD:refs/heads/main",
        ],
    );
    run_git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            remote.to_str().expect("UTF-8 remote path"),
        ],
    );

    let hostile_upload_pack = temp.path().join("hostile-upload-pack");
    let hostile_marker = temp.path().join("hostile-upload-pack-ran");
    write_executable(
        &hostile_upload_pack,
        "#!/usr/bin/env bash\nprintf reached > \"$HOSTILE_UPLOAD_PACK_MARKER\"\nexit 99\n",
    );
    run_git(
        &repo,
        &[
            "config",
            "remote.origin.uploadpack",
            hostile_upload_pack
                .to_str()
                .expect("UTF-8 hostile upload-pack path"),
        ],
    );

    let output = std::process::Command::new(&wrapper)
        .args([
            "ls-remote",
            "--heads",
            "origin",
            "refs/heads/main",
            "refs/heads/feat/comp-003-closed-operators",
        ])
        .current_dir(&repo)
        .env("CSA_REAL_GIT", "/usr/bin/git")
        .env("HOSTILE_UPLOAD_PACK_MARKER", &hostile_marker)
        .output_with_timeout()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("refs/heads/main"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        !hostile_marker.exists(),
        "named remote upload-pack override escaped the guard"
    );
}
