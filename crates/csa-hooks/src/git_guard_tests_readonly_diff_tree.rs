#[cfg(unix)]
#[test]
fn wrapper_allows_readonly_git_diff_tree_commit_path_audits() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("git");
    write_executable(&wrapper, git_wrapper_script());

    let fake_git = temp.path().join("real-git");
    write_executable(
        &fake_git,
        "#!/usr/bin/env bash\nif [ \"${1:-}\" = --exec-path ]; then exec /usr/bin/git --exec-path; fi\nprintf '%s\\n' \"$*\"\n",
    );

    let cases: &[&[&str]] = &[
        &["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"],
        &["diff-tree", "--no-commit-id", "--name-status", "-r", "HEAD"],
    ];

    for args in cases {
        let output = std::process::Command::new(&wrapper)
            .args(*args)
            .env("CSA_REAL_GIT", &fake_git)
            .env_remove("CSA_SESSION_DIR")
            .env_remove("CSA_GIT_PUSH_ALLOWED")
            .output_with_timeout()
            .unwrap();

        assert!(
            output.status.success(),
            "{}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            args.join(" ")
        );
    }
}

#[cfg(unix)]
#[test]
fn wrapper_still_blocks_network_push_without_git_fixtures() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("git");
    write_executable(&wrapper, git_wrapper_script());

    let marker = temp.path().join("fake-git-reached");
    let fake_git = temp.path().join("real-git");
    write_executable(
        &fake_git,
        r#"#!/usr/bin/env bash
if [ "${1:-}" = --exec-path ]; then exec /usr/bin/git --exec-path; fi
printf reached > "$FAKE_GIT_MARKER"
printf '%s\n' "$*"
"#,
    );

    let output = std::process::Command::new(&wrapper)
        .args([
            "push",
            "https://example.invalid/publication.git",
            "HEAD:main",
        ])
        .env("CSA_REAL_GIT", &fake_git)
        .env("FAKE_GIT_MARKER", &marker)
        .env_remove("CSA_SESSION_DIR")
        .env_remove("CSA_GIT_PUSH_ALLOWED")
        .output_with_timeout()
        .unwrap();

    assert_eq!(output.status.code(), Some(128));
    assert!(!marker.exists(), "fake Git reached for network push");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("CSA git-guard: blocked command: git"),
        "{stderr}"
    );
    assert!(stderr.contains("hermetic local bare fixture"), "{stderr}");
    assert!(
        !stderr.contains("example.invalid"),
        "destination leaked in {stderr}"
    );
    assert!(String::from_utf8_lossy(&output.stdout).trim().is_empty());
}
