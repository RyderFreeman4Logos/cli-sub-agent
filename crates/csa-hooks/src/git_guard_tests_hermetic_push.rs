#[cfg(unix)]
#[test]
fn wrapper_allows_real_push_to_session_owned_local_bare_fixture() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let session_dir = temp.path().join("session");
    let wrapper = session_dir.join("bin/git");
    std::fs::create_dir_all(wrapper.parent().expect("wrapper parent")).unwrap();
    write_executable(&wrapper, git_wrapper_script());

    let source = temp.path().join("source");
    init_worktree_repo(&source);
    let fixture_root = session_dir.join("git-fixtures");
    let bare = fixture_root.join("transport.git");
    init_bare_repo(&fixture_root, &bare);

    for (destination, reference) in [
        (
            bare.display().to_string(),
            "refs/heads/hermetic-path-fixture",
        ),
        (
            format!("file://{}", bare.display()),
            "refs/heads/hermetic-file-url-fixture",
        ),
    ] {
        let refspec = format!("HEAD:{reference}");
        let output = std::process::Command::new(&wrapper)
            .args(["push", destination.as_str(), refspec.as_str()])
            .current_dir(&source)
            .env("CSA_SESSION_DIR", &session_dir)
            .env_remove("CSA_GIT_PUSH_ALLOWED")
            .output()
            .expect("push to local bare fixture through guard");
        assert!(
            output.status.success(),
            "push to {destination} failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );

        let received = std::process::Command::new("git")
            .args([
                "-C",
                bare.to_str().expect("UTF-8 bare fixture path"),
                "rev-parse",
                "--verify",
                reference,
            ])
            .output()
            .expect("inspect received fixture ref");
        assert!(
            received.status.success(),
            "fixture ref {reference} was not received:\n{}",
            String::from_utf8_lossy(&received.stderr),
        );
    }
}

#[cfg(unix)]
#[test]
fn wrapper_blocks_remote_push_with_hermetic_fixture_diagnostic() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let session_dir = temp.path().join("session");
    let wrapper = session_dir.join("bin/git");
    std::fs::create_dir_all(wrapper.parent().expect("wrapper parent")).unwrap();
    std::fs::create_dir_all(session_dir.join("git-fixtures")).unwrap();
    write_executable(&wrapper, git_wrapper_script());

    let fake_git = temp.path().join("real-git");
    write_executable(&fake_git, "#!/usr/bin/env bash\necho \"$@\"\n");

    let output = std::process::Command::new(&wrapper)
        .args(["push", "https://example.invalid/publication.git", "HEAD:main"])
        .env("CSA_REAL_GIT", &fake_git)
        .env("CSA_SESSION_DIR", &session_dir)
        .env_remove("CSA_GIT_PUSH_ALLOWED")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("blocked command: git push https://example.invalid/publication.git HEAD:main"),
        "{stderr}"
    );
    assert!(stderr.contains("hermetic local bare fixture"), "{stderr}");
    assert!(
        stderr.contains("git-fixtures"),
        "diagnostic should name the session-owned fixture root: {stderr}"
    );
    assert!(String::from_utf8_lossy(&output.stdout).trim().is_empty());
}

#[cfg(unix)]
#[test]
fn wrapper_blocks_push_to_working_repository_git_dir() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let session_dir = temp.path().join("session");
    let wrapper = session_dir.join("bin/git");
    std::fs::create_dir_all(wrapper.parent().expect("wrapper parent")).unwrap();
    let fixture_root = session_dir.join("git-fixtures");
    std::fs::create_dir_all(&fixture_root).unwrap();
    write_executable(&wrapper, git_wrapper_script());

    // The destination is deliberately under the fixture root and is bare, so
    // this proves the separate worktree exclusion rather than fixture-root or
    // bare-repository rejection.
    let source = fixture_root.join("working-repository");
    init_worktree_repo(&source);
    let output = std::process::Command::new(&wrapper)
        .args([
            "push",
            source
                .join(".git")
                .to_str()
                .expect("UTF-8 working repository git dir"),
            "HEAD:refs/heads/should-not-publish",
        ])
        .current_dir(&source)
        .env("CSA_SESSION_DIR", &session_dir)
        .env_remove("CSA_GIT_PUSH_ALLOWED")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("blocked command: git push"), "{stderr}");
    assert!(stderr.contains("hermetic local bare fixture"), "{stderr}");

    let ref_check = std::process::Command::new("git")
        .args([
            "-C",
            source.to_str().expect("UTF-8 source path"),
            "show-ref",
            "--verify",
            "--quiet",
            "refs/heads/should-not-publish",
        ])
        .output()
        .expect("inspect rejected working-repository ref");
    assert_eq!(ref_check.status.code(), Some(1));
}

#[cfg(unix)]
#[test]
fn wrapper_blocks_fixture_path_that_canonicalizes_outside_session() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let session_dir = temp.path().join("session");
    let wrapper = session_dir.join("bin/git");
    let fixture_root = session_dir.join("git-fixtures");
    std::fs::create_dir_all(wrapper.parent().expect("wrapper parent")).unwrap();
    std::fs::create_dir_all(&fixture_root).unwrap();
    write_executable(&wrapper, git_wrapper_script());

    let source = temp.path().join("source");
    init_worktree_repo(&source);
    let outside_bare = temp.path().join("outside.git");
    init_bare_repo(temp.path(), &outside_bare);
    let escaped_fixture = fixture_root.join("escaped.git");
    std::os::unix::fs::symlink(&outside_bare, &escaped_fixture).unwrap();

    let output = std::process::Command::new(&wrapper)
        .args([
            "push",
            escaped_fixture.to_str().expect("UTF-8 escaped fixture path"),
            "HEAD:refs/heads/should-not-publish",
        ])
        .current_dir(&source)
        .env("CSA_SESSION_DIR", &session_dir)
        .env_remove("CSA_GIT_PUSH_ALLOWED")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(128));
    let ref_check = std::process::Command::new("git")
        .args([
            "-C",
            outside_bare.to_str().expect("UTF-8 outside bare path"),
            "show-ref",
            "--verify",
            "--quiet",
            "refs/heads/should-not-publish",
        ])
        .output()
        .expect("inspect rejected escaped-fixture ref");
    assert_eq!(ref_check.status.code(), Some(1));
}
