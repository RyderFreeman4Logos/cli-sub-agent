#[cfg(unix)]
#[test]
fn wrapper_allows_exact_git_version_grammar() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("git");
    write_executable(&wrapper, git_wrapper_script());

    let fake_git = temp.path().join("real-git");
    write_executable(&fake_git, "#!/usr/bin/env bash\nprintf '%s\\n' \"$*\"\n");

    for args in [&["version"][..], &["--version"][..]] {
        let output = std::process::Command::new(&wrapper)
            .args(args)
            .env("CSA_REAL_GIT", &fake_git)
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
fn wrapper_allows_exact_git_init_grammar() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("git");
    write_executable(&wrapper, git_wrapper_script());

    let fake_git = temp.path().join("real-git");
    write_executable(&fake_git, "#!/usr/bin/env bash\nprintf '%s\\n' \"$*\"\n");

    for args in [&["init"][..], &["init", "-q"][..], &["init", "--quiet"][..]] {
        let output = std::process::Command::new(&wrapper)
            .args(args)
            .env("CSA_REAL_GIT", &fake_git)
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
fn wrapper_rejects_git_init_option_smuggling() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("git");
    write_executable(&wrapper, git_wrapper_script());

    let marker = temp.path().join("fake-git-reached");
    let fake_git = temp.path().join("real-git");
    write_executable(
        &fake_git,
        "#!/usr/bin/env bash\nprintf reached > \"$FAKE_GIT_MARKER\"\nprintf '%s\\n' \"$*\"\n",
    );

    let cases: &[(&[&str], &[&str])] = &[
        (&["init", "secret-operand"], &["secret-operand"]),
        (&["init", "-q", "secret-operand"], &["secret-operand"]),
        (&["init", "--quiet", "secret-operand"], &["secret-operand"]),
        (&["init", "--bare"], &[]),
        (
            &["init", "--template=/tmp/secret-template"],
            &["secret-template"],
        ),
        (
            &["init", "--separate-git-dir=/tmp/secret-git-dir"],
            &["secret-git-dir"],
        ),
        (
            &["init", "--initial-branch=secret-branch"],
            &["secret-branch"],
        ),
        (
            &["-C", "/tmp/secret-worktree", "init"],
            &["secret-worktree"],
        ),
        (
            &["--git-dir=/tmp/secret-git-dir", "init"],
            &["secret-git-dir"],
        ),
        (
            &["-c", "alias.bootstrap=init", "bootstrap"],
            &["alias.bootstrap=init", "bootstrap"],
        ),
        (&["version", "secret-extra"], &["secret-extra"]),
        (&["--version", "secret-extra"], &["secret-extra"]),
        (&["--version=secret-extra"], &["secret-extra"]),
        (&["--version", "--help"], &[]),
        (
            &["-C", "/tmp/secret-worktree", "--version"],
            &["secret-worktree"],
        ),
        (
            &["--git-dir=/tmp/secret-git-dir", "--version"],
            &["secret-git-dir"],
        ),
    ];

    for (args, secrets) in cases {
        let _ = std::fs::remove_file(&marker);
        let output = std::process::Command::new(&wrapper)
            .args(*args)
            .env("CSA_REAL_GIT", &fake_git)
            .env("FAKE_GIT_MARKER", &marker)
            .output_with_timeout()
            .unwrap();

        assert_eq!(output.status.code(), Some(128), "{}", args.join(" "));
        assert!(
            String::from_utf8_lossy(&output.stdout).trim().is_empty(),
            "fake Git stdout for {}",
            args.join(" ")
        );
        assert!(!marker.exists(), "fake Git reached for {}", args.join(" "));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("CSA git-guard: blocked command: git"),
            "{}: {stderr}",
            args.join(" ")
        );
        assert!(
            stderr.contains("hermetic local bare fixture"),
            "{}: {stderr}",
            args.join(" ")
        );
        for secret in *secrets {
            assert!(!stderr.contains(secret), "{secret} leaked in {stderr}");
        }
    }
}
