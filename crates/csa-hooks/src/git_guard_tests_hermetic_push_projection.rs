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
            "git@scm.invalid:private/path?token=query-secret\nforged-log-line",
            "HEAD:refs/heads/main",
        ])
        .env("CSA_SESSION_DIR", &session_dir)
        .env_remove("CSA_GIT_PUSH_ALLOWED")
        .output_with_timeout()
        .expect("credential-bearing blocked push through guard");

    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("blocked command: git"), "{stderr}");
    for secret in [
        "configuration-secret",
        "private/path",
        "query-secret",
        "git@scm.invalid",
        "\nforged-log-line",
    ] {
        assert!(!stderr.contains(secret), "diagnostic leaked {secret:?}: {stderr}");
    }
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
fn wrapper_ignores_source_push_instead_of_rewrite_for_projected_fixture_push() {
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

    assert!(
        output.status.success(),
        "source pushInsteadOf rewrite escaped projection:\nstdout:\n{}\nstderr:\n{}",
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
        .expect("inspect pushInsteadOf outside sentinel");
    assert_eq!(
        received.status.code(),
        Some(1),
        "source pushInsteadOf rewrite unexpectedly updated outside sentinel",
    );
    let fixture_ref = std::process::Command::new("git")
        .args([
            "-C",
            fixture.to_str().expect("UTF-8 fixture path"),
            "show-ref",
            "--verify",
            "--quiet",
            reference,
        ])
        .output_with_timeout()
        .expect("inspect projected fixture ref");
    assert!(
        fixture_ref.status.success(),
        "projected fixture did not receive {reference}"
    );
}

#[cfg(unix)]
#[test]
fn wrapper_blocks_alias_and_publication_plumbing_surfaces() {
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
    let reference = "refs/heads/alias-or-plumbing-must-not-publish";
    let refspec = format!("HEAD:{reference}");
    run_git(&source, &["config", "alias.publish", "push"]);

    for args in [
        vec![
            "publish".to_owned(),
            fixture.display().to_string(),
            refspec.clone(),
        ],
        vec![
            "-c".to_owned(),
            "alias.publish=push".to_owned(),
            "publish".to_owned(),
            fixture.display().to_string(),
            refspec.clone(),
        ],
        vec!["send-pack".to_owned(), fixture.display().to_string()],
        vec!["receive-pack".to_owned(), fixture.display().to_string()],
        vec!["remote".to_owned(), "update".to_owned()],
    ] {
        let output = std::process::Command::new(&wrapper)
            .args(&args)
            .current_dir(&source)
            .env("CSA_SESSION_DIR", &session_dir)
            .env_remove("CSA_GIT_PUSH_ALLOWED")
            .output_with_timeout()
            .expect("publication-capable command through guard");
        assert_eq!(
            output.status.code(),
            Some(128),
            "publication-capable command escaped guard: {args:?}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr),
        );
    }

    let fixture_ref = std::process::Command::new("git")
        .args([
            "-C",
            fixture.to_str().expect("UTF-8 fixture path"),
            "show-ref",
            "--verify",
            "--quiet",
            reference,
        ])
        .output_with_timeout()
        .expect("inspect alias fixture sentinel");
    assert_eq!(fixture_ref.status.code(), Some(1));
}

#[cfg(unix)]
#[test]
fn wrapper_ignores_path_named_remote_pushurl_and_spaced_rewrite_config() {
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
    let outside_bare = temp.path().join("outside rewrite.git");
    init_bare_repo(temp.path(), &outside_bare);

    let pushurl_key = format!(r#"remote."{}".pushurl"#, fixture.display());
    run_git(
        &source,
        &[
            "config",
            pushurl_key.as_str(),
            outside_bare.to_str().expect("UTF-8 outside bare path"),
        ],
    );
    let rewrite_key = format!(r#"url."{}".insteadOf"#, outside_bare.display());
    run_git(
        &source,
        &[
            "config",
            rewrite_key.as_str(),
            fixture.to_str().expect("UTF-8 fixture path"),
        ],
    );

    let reference = "refs/heads/configless-projection";
    let refspec = format!("HEAD:{reference}");
    let output = std::process::Command::new(&wrapper)
        .args([
            "push",
            fixture.to_str().expect("UTF-8 fixture path"),
            refspec.as_str(),
        ])
        .current_dir(&source)
        .env("CSA_SESSION_DIR", &session_dir)
        .env_remove("CSA_GIT_PUSH_ALLOWED")
        .output_with_timeout()
        .expect("configured fixture push through guard");
    assert!(
        output.status.success(),
        "projection used source remote/rewrite config:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    for (repository, expected, label) in [
        (&fixture, true, "fixture"),
        (&outside_bare, false, "outside sentinel"),
    ] {
        let ref_check = std::process::Command::new("git")
            .args([
                "-C",
                repository.to_str().expect("UTF-8 bare path"),
                "show-ref",
                "--verify",
                "--quiet",
                reference,
            ])
            .output_with_timeout()
            .expect("inspect config projection destination");
        assert_eq!(
            ref_check.status.success(),
            expected,
            "unexpected ref state in {label}"
        );
    }
}

#[cfg(unix)]
#[test]
fn wrapper_pins_trusted_git_exec_path_for_projected_push() {
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
    let hostile_exec_path = temp.path().join("hostile-git-exec-path");
    std::fs::create_dir_all(&hostile_exec_path).expect("create hostile git exec path");
    let hostile_marker = temp.path().join("hostile-helper-ran");
    write_executable(
        &hostile_exec_path.join("git-receive-pack"),
        "#!/bin/sh\nprintf hostile > \"$CSA_GIT_GUARD_HOSTILE_MARKER\"\nexit 91\n",
    );

    let reference = "refs/heads/trusted-exec-path";
    let output = std::process::Command::new(&wrapper)
        .args([
            "push",
            fixture.to_str().expect("UTF-8 fixture path"),
            "HEAD:refs/heads/trusted-exec-path",
        ])
        .current_dir(&source)
        .env("CSA_SESSION_DIR", &session_dir)
        .env("GIT_EXEC_PATH", &hostile_exec_path)
        .env("CSA_GIT_GUARD_HOSTILE_MARKER", &hostile_marker)
        .env_remove("CSA_GIT_PUSH_ALLOWED")
        .output_with_timeout()
        .expect("hostile helper fixture push through guard");
    assert!(
        output.status.success(),
        "hostile GIT_EXEC_PATH controlled projected push:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        !hostile_marker.exists(),
        "hostile git-receive-pack helper was invoked"
    );
    let fixture_ref = std::process::Command::new("git")
        .args([
            "-C",
            fixture.to_str().expect("UTF-8 fixture path"),
            "show-ref",
            "--verify",
            "--quiet",
            reference,
        ])
        .output_with_timeout()
        .expect("inspect trusted exec path fixture ref");
    assert!(fixture_ref.status.success());
}

#[cfg(unix)]
#[test]
fn wrapper_keeps_descriptor_pinned_across_final_destination_swap() {
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
    let held_fixture = fixture_root.join("held.git");
    let outside_bare = temp.path().join("outside.git");
    init_bare_repo(temp.path(), &outside_bare);
    let swapping_git = temp.path().join("swapping-real-git");
    write_executable(
        &swapping_git,
        "#!/bin/sh\nif [ \"${2:-}\" = push ] && [ -n \"${CSA_GIT_GUARD_SWAP_FROM:-}\" ]; then\n  mv \"$CSA_GIT_GUARD_SWAP_FROM\" \"$CSA_GIT_GUARD_HELD\"\n  ln -s \"$CSA_GIT_GUARD_OUTSIDE\" \"$CSA_GIT_GUARD_SWAP_FROM\"\nfi\nexec /usr/bin/git \"$@\"\n",
    );

    let reference = "refs/heads/descriptor-pinned";
    let output = std::process::Command::new(&wrapper)
        .args([
            "push",
            fixture.to_str().expect("UTF-8 fixture path"),
            "HEAD:refs/heads/descriptor-pinned",
        ])
        .current_dir(&source)
        .env("CSA_REAL_GIT", &swapping_git)
        .env("CSA_SESSION_DIR", &session_dir)
        .env("CSA_GIT_GUARD_SWAP_FROM", &fixture)
        .env("CSA_GIT_GUARD_HELD", &held_fixture)
        .env("CSA_GIT_GUARD_OUTSIDE", &outside_bare)
        .env_remove("CSA_GIT_PUSH_ALLOWED")
        .output_with_timeout()
        .expect("racing fixture push through guard");
    assert!(
        output.status.success(),
        "descriptor-pinned push failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    for (repository, expected, label) in [
        (&held_fixture, true, "descriptor-pinned fixture"),
        (&outside_bare, false, "outside swap target"),
    ] {
        let ref_check = std::process::Command::new("git")
            .args([
                "-C",
                repository.to_str().expect("UTF-8 bare path"),
                "show-ref",
                "--verify",
                "--quiet",
                reference,
            ])
            .output_with_timeout()
            .expect("inspect post-swap destination");
        assert_eq!(
            ref_check.status.success(),
            expected,
            "unexpected ref state in {label}"
        );
    }
}
