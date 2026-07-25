// Focused adversarial regressions for the fail-closed hermetic push projection.
// This file is included from git_guard_tests.rs and intentionally shares its
// fixture helpers, timeout wrapper, and process-wide environment lock.

#[cfg(unix)]
#[test]
fn wrapper_redacts_credentials_and_control_bytes_in_blocked_push_diagnostic() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let session_dir = temp.path().join("session");
    let wrapper = session_dir.join("bin/git");
    std::fs::create_dir_all(wrapper.parent().expect("wrapper parent")).unwrap();
    std::fs::create_dir_all(session_dir.join("git-fixtures")).unwrap();
    write_executable(&wrapper, git_wrapper_script());

    let output = std::process::Command::new(&wrapper)
        .args([
            "-c",
            "http.extraHeader=Authorization: Bearer configuration-secret",
            "push",
            "https://alice:url-secret@example.invalid/publication.git\nforged-log-line",
            "HEAD:refs/heads/main",
        ])
        .env("CSA_SESSION_DIR", &session_dir)
        .env_remove("CSA_GIT_PUSH_ALLOWED")
        .output_with_timeout()
        .expect("credential-bearing blocked push through guard");

    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("blocked command: git"), "{stderr}");
    assert!(!stderr.contains("configuration-secret"), "{stderr}");
    assert!(!stderr.contains("url-secret"), "{stderr}");
    assert!(!stderr.contains("alice:"), "{stderr}");
    assert!(!stderr.contains("\nforged-log-line"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn wrapper_blocks_closed_push_grammar_after_destination() {
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
    let destination = bare.to_str().expect("UTF-8 bare fixture path");

    for (refspec, trailing, reference) in [
        (
            "HEAD:refs/heads/mirror",
            "--mirror",
            "refs/heads/mirror",
        ),
        (
            "HEAD:refs/heads/delete",
            "--delete",
            "refs/heads/delete",
        ),
        (
            "HEAD:refs/heads/no-verify",
            "--no-verify",
            "refs/heads/no-verify",
        ),
        (
            "HEAD:refs/heads/combined-force",
            "-fv",
            "refs/heads/combined-force",
        ),
        (
            "HEAD:refs/heads/valued-force",
            "--force-with-lease=refs/heads/main",
            "refs/heads/valued-force",
        ),
    ] {
        let output = std::process::Command::new(&wrapper)
            .args(["push", destination, refspec, trailing])
            .current_dir(&source)
            .env("CSA_SESSION_DIR", &session_dir)
            .env_remove("CSA_GIT_PUSH_ALLOWED")
            .output_with_timeout()
            .expect("closed grammar push through guard");

        assert_eq!(
            output.status.code(),
            Some(128),
            "open push grammar {refspec} {trailing} was not blocked:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );

        let received = std::process::Command::new("git")
            .args([
                "-C",
                destination,
                "show-ref",
                "--verify",
                "--quiet",
                reference,
            ])
            .output_with_timeout()
            .expect("inspect rejected closed-grammar ref");
        assert_eq!(
            received.status.code(),
            Some(1),
            "open push grammar unexpectedly updated {reference}",
        );
    }
}

#[cfg(unix)]
#[test]
fn wrapper_executes_the_canonical_projected_fixture_destination() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let session_dir = temp.path().join("session");
    let wrapper = session_dir.join("bin/git");
    std::fs::create_dir_all(wrapper.parent().expect("wrapper parent")).unwrap();
    write_executable(&wrapper, git_wrapper_script());

    let source = temp.path().join("source");
    init_worktree_repo(&source);
    let fixture_root = session_dir.join("git-fixtures");
    let fixture = fixture_root.join("transport.git");
    init_bare_repo(&fixture_root, &fixture);
    let outside_bare = temp.path().join("outside.git");
    init_bare_repo(temp.path(), &outside_bare);

    let dotted_destination = fixture_root.join(".").join("transport.git");
    let dotted_destination = dotted_destination
        .to_str()
        .expect("UTF-8 dotted fixture path")
        .to_owned();
    assert_ne!(dotted_destination, fixture.display().to_string());
    let rewrite_config = format!("url.{}.insteadOf", outside_bare.display());
    run_git(
        &source,
        &["config", rewrite_config.as_str(), dotted_destination.as_str()],
    );

    let reference = "refs/heads/projected-canonical-destination";
    let output = std::process::Command::new(&wrapper)
        .args([
            "push",
            dotted_destination.as_str(),
            "HEAD:refs/heads/projected-canonical-destination",
        ])
        .current_dir(&source)
        .env("CSA_SESSION_DIR", &session_dir)
        .env_remove("CSA_GIT_PUSH_ALLOWED")
        .output_with_timeout()
        .expect("projected canonical fixture push through guard");
    assert!(
        output.status.success(),
        "canonical destination projection failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    for (repository, expected) in [(&fixture, true), (&outside_bare, false)] {
        let received = std::process::Command::new("git")
            .args([
                "-C",
                repository.to_str().expect("UTF-8 bare repository path"),
                "show-ref",
                "--verify",
                "--quiet",
                reference,
            ])
            .output_with_timeout()
            .expect("inspect projected destination ref");
        assert_eq!(
            received.status.success(),
            expected,
            "canonical projection delivered ref to unexpected repository {}",
            repository.display(),
        );
    }
}

#[cfg(unix)]
#[test]
fn wrapper_blocks_invocation_scoped_url_rewrites_without_touching_outside_sentinel() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");

    for rewrite_kind in ["-c", "--config-env"] {
        let temp = tempfile::tempdir().unwrap();
        let session_dir = temp.path().join("session");
        let wrapper = session_dir.join("bin/git");
        std::fs::create_dir_all(wrapper.parent().expect("wrapper parent")).unwrap();
        write_executable(&wrapper, git_wrapper_script());

        let source = temp.path().join("source");
        init_worktree_repo(&source);
        let fixture_root = session_dir.join("git-fixtures");
        let fixture = fixture_root.join("transport.git");
        init_bare_repo(&fixture_root, &fixture);
        let outside_bare = temp.path().join("outside.git");
        init_bare_repo(temp.path(), &outside_bare);
        let reference = format!("refs/heads/should-not-publish-{rewrite_kind}");
        let rewrite_config = format!(
            "url.{}.insteadOf={}",
            outside_bare.display(),
            fixture.display()
        );

        let mut command = std::process::Command::new(&wrapper);
        command.current_dir(&source).env("CSA_SESSION_DIR", &session_dir);
        match rewrite_kind {
            "-c" => {
                command.args(["-c", rewrite_config.as_str()]);
            }
            "--config-env" => {
                let config_env = format!(
                    "url.{}.insteadOf=CSA_TEST_REWRITE_PREFIX",
                    outside_bare.display()
                );
                command
                    .args(["--config-env", config_env.as_str()])
                    .env("CSA_TEST_REWRITE_PREFIX", fixture.as_os_str());
            }
            _ => unreachable!("test matrix is closed"),
        }
        let refspec = format!("HEAD:{reference}");
        let output = command
            .args([
                "push",
                fixture.to_str().expect("UTF-8 fixture path"),
                refspec.as_str(),
            ])
            .env_remove("CSA_GIT_PUSH_ALLOWED")
            .output_with_timeout()
            .expect("invocation-scoped rewrite push through guard");

        assert_eq!(
            output.status.code(),
            Some(128),
            "{rewrite_kind} rewrite was not blocked:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );

        let received = std::process::Command::new("git")
            .args([
                "-C",
                outside_bare.to_str().expect("UTF-8 outside bare path"),
                "show-ref",
                "--verify",
                "--quiet",
                reference.as_str(),
            ])
            .output_with_timeout()
            .expect("inspect blocked invocation-scoped rewrite sentinel");
        assert_eq!(
            received.status.code(),
            Some(1),
            "{rewrite_kind} rewrite unexpectedly updated outside sentinel",
        );
    }
}

#[cfg(unix)]
#[test]
fn wrapper_blocks_push_instead_of_rewrite_without_touching_outside_sentinel() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let session_dir = temp.path().join("session");
    let wrapper = session_dir.join("bin/git");
    std::fs::create_dir_all(wrapper.parent().expect("wrapper parent")).unwrap();
    write_executable(&wrapper, git_wrapper_script());

    let source = temp.path().join("source");
    init_worktree_repo(&source);
    let fixture_root = session_dir.join("git-fixtures");
    let fixture = fixture_root.join("transport.git");
    init_bare_repo(&fixture_root, &fixture);
    let outside_bare = temp.path().join("outside.git");
    init_bare_repo(temp.path(), &outside_bare);
    let rewrite_config = format!("url.{}.pushInsteadOf", outside_bare.display());
    run_git(
        &source,
        &[
            "config",
            rewrite_config.as_str(),
            fixture.to_str().expect("UTF-8 fixture path"),
        ],
    );

    let reference = "refs/heads/should-not-publish-push-instead-of";
    let output = std::process::Command::new(&wrapper)
        .args([
            "push",
            fixture.to_str().expect("UTF-8 fixture path"),
            "HEAD:refs/heads/should-not-publish-push-instead-of",
        ])
        .current_dir(&source)
        .env("CSA_SESSION_DIR", &session_dir)
        .env_remove("CSA_GIT_PUSH_ALLOWED")
        .output_with_timeout()
        .expect("pushInsteadOf rewrite push through guard");

    assert_eq!(
        output.status.code(),
        Some(128),
        "pushInsteadOf rewrite was not blocked:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let received = std::process::Command::new("git")
        .args([
            "-C",
            outside_bare.to_str().expect("UTF-8 outside bare path"),
            "show-ref",
            "--verify",
            "--quiet",
            reference,
        ])
        .output_with_timeout()
        .expect("inspect blocked pushInsteadOf rewrite sentinel");
    assert_eq!(
        received.status.code(),
        Some(1),
        "pushInsteadOf rewrite unexpectedly updated outside sentinel",
    );
}
