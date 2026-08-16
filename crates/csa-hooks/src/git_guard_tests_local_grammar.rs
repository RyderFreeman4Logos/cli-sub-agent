#[cfg(unix)]
#[test]
fn wrapper_allows_exact_git_version_grammar() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("git");
    write_executable(&wrapper, git_wrapper_script());

    let fake_git = temp.path().join("real-git");
    write_executable(
        &fake_git,
        "#!/usr/bin/env bash\nif [ \"${1:-}\" = --exec-path ]; then exec /usr/bin/git --exec-path; fi\nprintf '%s\\n' \"$*\"\n",
    );

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
    write_executable(
        &fake_git,
        "#!/usr/bin/env bash\nif [ \"${1:-}\" = --exec-path ]; then exec /usr/bin/git --exec-path; fi\nprintf '%s\\n' \"$*\"\n",
    );

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
fn wrapper_isolates_exact_git_init_from_hostile_environment() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("git");
    write_executable(&wrapper, git_wrapper_script());

    let launcher = temp.path().join("launch-git-guard");
    write_executable(
        &launcher,
        r#"#!/bin/sh
set -eu
case "${HOSTILE_GIT_FAMILY}" in
  repository-path) export GIT_DIR="${HOSTILE_GIT_VALUE}" ;;
  template) export GIT_TEMPLATE_DIR="${HOSTILE_GIT_VALUE}" ;;
  config)
    export GIT_CONFIG_COUNT=1
    export GIT_CONFIG_KEY_0=init.templateDir
    export GIT_CONFIG_VALUE_0="${HOSTILE_GIT_VALUE}"
    ;;
esac
exec "${GIT_GUARD_UNDER_TEST}" "$@"
"#,
    );

    let mut altered = Vec::new();
    for family in ["repository-path", "template", "config"] {
        let repo = temp.path().join(format!("repo-{family}"));
        std::fs::create_dir(&repo).unwrap();
        let hostile = temp.path().join(format!("hostile-{family}"));
        if family != "repository-path" {
            std::fs::create_dir(&hostile).unwrap();
            std::fs::write(hostile.join("ambient-template-marker"), "hostile\n").unwrap();
        }

        let output = std::process::Command::new(&launcher)
            .arg("init")
            .current_dir(&repo)
            .env("CSA_REAL_GIT", "/usr/bin/git")
            .env("GIT_GUARD_UNDER_TEST", &wrapper)
            .env("HOSTILE_GIT_FAMILY", family)
            .env("HOSTILE_GIT_VALUE", &hostile)
            .output_with_timeout()
            .unwrap();

        assert!(
            output.status.success(),
            "{family}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let initialized_here = repo.join(".git").is_dir();
        let escaped = family == "repository-path" && hostile.exists();
        let template_applied = repo.join(".git/ambient-template-marker").exists();
        if !initialized_here || escaped || template_applied {
            altered.push(family);
        }
    }

    assert!(
        altered.is_empty(),
        "hostile Git environment altered exact local init: {}",
        altered.join(", ")
    );
}

#[cfg(unix)]
#[test]
fn wrapper_scrubs_init_defaults_and_trace_environment() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("git");
    write_executable(&wrapper, git_wrapper_script());

    let control_repo = temp.path().join("control");
    let hostile_repo = temp.path().join("hostile");
    std::fs::create_dir(&control_repo).unwrap();
    std::fs::create_dir(&hostile_repo).unwrap();

    let control = std::process::Command::new(&wrapper)
        .arg("init")
        .current_dir(&control_repo)
        .env("CSA_REAL_GIT", "/usr/bin/git")
        .output_with_timeout()
        .unwrap();
    assert!(
        control.status.success(),
        "control: {}",
        String::from_utf8_lossy(&control.stderr)
    );

    let trace = temp.path().join("external-git-trace");
    let launcher = temp.path().join("launch-hostile-git-guard");
    write_executable(
        &launcher,
        r#"#!/bin/sh
set -eu
export GIT_DEFAULT_HASH=sha256
export GIT_DEFAULT_REF_FORMAT=reftable
export GIT_TEST_DEFAULT_INITIAL_BRANCH_NAME=hostile-branch
export GIT_TRACE="${HOSTILE_GIT_TRACE}"
export GIT_TRACE2_EVENT="${HOSTILE_GIT_TRACE}"
exec "${GIT_GUARD_UNDER_TEST}" "$@"
"#,
    );
    let hostile = std::process::Command::new(&launcher)
        .arg("init")
        .current_dir(&hostile_repo)
        .env("CSA_REAL_GIT", "/usr/bin/git")
        .env("GIT_GUARD_UNDER_TEST", &wrapper)
        .env("HOSTILE_GIT_TRACE", &trace)
        .output_with_timeout()
        .unwrap();
    assert!(
        hostile.status.success(),
        "hostile: {}",
        String::from_utf8_lossy(&hostile.stderr)
    );

    let format_changed = std::fs::read(control_repo.join(".git/config")).unwrap()
        != std::fs::read(hostile_repo.join(".git/config")).unwrap();
    let branch_changed = std::fs::read(control_repo.join(".git/HEAD")).unwrap()
        != std::fs::read(hostile_repo.join(".git/HEAD")).unwrap();
    let trace_written = trace.exists();
    assert!(
        !format_changed && !branch_changed && !trace_written,
        "hostile Git environment altered exact local init: \
         format_changed={format_changed}, branch_changed={branch_changed}, \
         trace_written={trace_written}"
    );
}

#[cfg(unix)]
#[test]
fn wrapper_isolates_exec_path_probe_from_global_trace2_config() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("git");
    write_executable(&wrapper, git_wrapper_script());

    let home = temp.path().join("home");
    let repo = temp.path().join("repo");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&repo).unwrap();
    let trace = temp.path().join("external-trace2-event");
    std::fs::write(
        home.join(".gitconfig"),
        format!("[trace2]\n\teventTarget = {}\n", trace.display()),
    )
    .unwrap();

    let output = std::process::Command::new(&wrapper)
        .arg("init")
        .current_dir(&repo)
        .env("CSA_REAL_GIT", "/usr/bin/git")
        .env("HOME", &home)
        .output_with_timeout()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(repo.join(".git").is_dir(), "exact local init did not run");
    assert!(
        !trace.exists(),
        "exec-path probe wrote hostile global Trace2 target: {}",
        trace.display()
    );
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
