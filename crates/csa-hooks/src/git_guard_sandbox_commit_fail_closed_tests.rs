use super::*;
use crate::git_guard::SANDBOX_COMMIT_FAILURE_MARKER_FILE;

struct SandboxCommitFixture {
    _temp: tempfile::TempDir,
    session_dir: std::path::PathBuf,
    wrapper: std::path::PathBuf,
    repo: std::path::PathBuf,
    hook_count: std::path::PathBuf,
}

impl SandboxCommitFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let session_dir = temp.path().join("session");
        let wrapper = session_dir.join("bin/git");
        std::fs::create_dir_all(wrapper.parent().expect("wrapper parent"))
            .expect("create wrapper dir");
        write_executable(&wrapper, git_wrapper_script());
        let repo = temp.path().join("repo");
        init_worktree_repo(&repo);
        std::fs::write(repo.join("fixture.txt"), "staged change\n").expect("write staged file");
        run_git(&repo, &["add", "fixture.txt"]);
        let hook_count = temp.path().join("hook-count");
        write_executable(
            &repo.join(".git/hooks/pre-commit"),
            r#"#!/bin/sh
count=0
[ ! -f "${HOOK_COUNT}" ] || count="$(cat "${HOOK_COUNT}")"
printf '%s\n' "$((count + 1))" > "${HOOK_COUNT}"
echo "hook cannot write /var/tmp: Read-only file system" >&2
exit 1
"#,
        );
        Self {
            _temp: temp,
            session_dir,
            wrapper,
            repo,
            hook_count,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.wrapper);
        command
            .args(["commit", "-m", "sandbox commit"])
            .current_dir(&self.repo)
            .env("CSA_REAL_GIT", "/usr/bin/git")
            .env("CSA_FS_SANDBOXED", "1")
            .env("CSA_SESSION_DIR", &self.session_dir)
            .env("HOOK_COUNT", &self.hook_count);
        command
    }

    fn run(&self) -> Output {
        self.command()
            .output_with_timeout()
            .expect("run guarded commit")
    }

    fn marker(&self) -> std::path::PathBuf {
        self.session_dir.join(SANDBOX_COMMIT_FAILURE_MARKER_FILE)
    }

    fn hook_count(&self) -> u32 {
        std::fs::read_to_string(&self.hook_count)
            .ok()
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(0)
    }
}

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

#[test]
fn wrapper_rejects_malformed_markers_without_rerunning_hooks() {
    use std::os::unix::fs::symlink;

    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    for marker_kind in ["symlink", "oversized", "multi-record"] {
        let fixture = SandboxCommitFixture::new();
        let first = fixture.run();
        assert!(!first.status.success());
        assert_eq!(
            fixture.hook_count(),
            1,
            "first attempt: {}",
            String::from_utf8_lossy(&first.stderr)
        );
        let marker = fixture.marker();
        let valid_record = std::fs::read(&marker).expect("read valid marker");
        match marker_kind {
            "symlink" => {
                let target = marker.with_extension("target");
                std::fs::rename(&marker, &target).expect("move marker");
                symlink(target, &marker).expect("symlink marker");
            }
            "oversized" => {
                let file = std::fs::OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(&marker)
                    .expect("open marker");
                file.set_len(1024 * 1024).expect("grow marker");
            }
            "multi-record" => {
                let mut records = valid_record;
                records.extend_from_slice(b"trailing record\n");
                std::fs::write(&marker, records).expect("write multi-record marker");
            }
            _ => unreachable!(),
        }

        let output = fixture.run();
        assert!(!output.status.success(), "{marker_kind}");
        assert_eq!(
            fixture.hook_count(),
            1,
            "{marker_kind} marker must not rerun the hook"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("fingerprint state is unavailable"),
            "{marker_kind}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn rejecting_helper_body(message: &str) -> String {
    format!(
        "#!/bin/sh\ncount=0\n[ ! -f \"${{HOOK_COUNT}}\" ] || count=\"$(cat \"${{HOOK_COUNT}}\")\"\nprintf '%s\\n' \"$((count + 1))\" > \"${{HOOK_COUNT}}\"\necho '{message}' >&2\nexit 1\n"
    )
}

#[test]
fn wrapper_fingerprints_supported_hook_helpers() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    for helper_kind in ["untracked", "nested", "hidden", "outside"] {
        let fixture = SandboxCommitFixture::new();
        let helper = match helper_kind {
            "untracked" => fixture.repo.join("hook-helper.sh"),
            "nested" => fixture.repo.join(".git/hooks/lib/check.sh"),
            "hidden" => fixture.repo.join(".git/hooks/.check"),
            "outside" => fixture
                .session_dir
                .parent()
                .expect("fixture root")
                .join("outside-helper.sh"),
            _ => unreachable!(),
        };
        std::fs::create_dir_all(helper.parent().expect("helper parent"))
            .expect("create helper parent");
        write_executable(&helper, rejecting_helper_body("first rejection"));
        write_executable(
            &fixture.repo.join(".git/hooks/pre-commit"),
            "#!/bin/sh\nexec \"${HOOK_HELPER}\"\n",
        );
        let declared = matches!(helper_kind, "untracked" | "outside");
        let run = || {
            let mut command = fixture.command();
            command.env("HOOK_HELPER", &helper);
            if declared {
                command.env("CSA_GIT_GUARD_HOOK_HELPERS", &helper);
            }
            command
                .output_with_timeout()
                .expect("run helper-backed commit")
        };

        assert!(!run().status.success());
        assert_eq!(fixture.hook_count(), 1, "{helper_kind}");
        write_executable(&helper, rejecting_helper_body("repaired rejection"));
        assert!(!run().status.success());
        assert_eq!(
            fixture.hook_count(),
            2,
            "changed {helper_kind} helper must allow one fresh hook attempt"
        );
    }
}

#[test]
fn wrapper_fails_closed_when_fingerprint_producers_fail() {
    use std::os::unix::fs::PermissionsExt;

    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    for producer in [
        "env",
        "sort",
        "head",
        "config",
        "diff",
        "diff-timeout",
        "hooks",
    ] {
        let fixture = SandboxCommitFixture::new();
        let fake_bin = fixture
            .session_dir
            .parent()
            .expect("fixture root")
            .join("fake-bin");
        std::fs::create_dir(&fake_bin).expect("create fake bin");
        let fake_git = fake_bin.join("git-producer");
        std::fs::write(
            &fake_git,
            r#"#!/bin/sh
case "${FAIL_PRODUCER}:$*" in
  "head:rev-parse --verify HEAD") exit 42 ;;
  "config:config --null --list --show-origin"|diff:*" diff --no-ext-diff --no-textconv --binary --") exit 42 ;;
  diff-timeout:*" diff --no-ext-diff --no-textconv --binary --") exec /usr/bin/sleep 5 ;;
esac
exec /usr/bin/git "$@"
"#,
        )
        .expect("write producer-failing Git");
        std::fs::set_permissions(&fake_git, std::fs::Permissions::from_mode(0o755))
            .expect("make fake Git executable");
        for utility in ["env", "sort", "find"] {
            let fake_utility = fake_bin.join(utility);
            let body = if producer == utility || (producer == "hooks" && utility == "find") {
                "#!/bin/sh\nexit 42\n"
            } else {
                match utility {
                    "env" => "#!/bin/sh\nexec /usr/bin/env \"$@\"\n",
                    "sort" => "#!/bin/sh\nexec /usr/bin/sort \"$@\"\n",
                    "find" => "#!/bin/sh\nexec /usr/bin/find \"$@\"\n",
                    _ => unreachable!(),
                }
            };
            write_executable(&fake_utility, body);
        }
        let original_path = std::env::var_os("PATH").unwrap_or_default();
        let mut path = fake_bin.into_os_string();
        path.push(":");
        path.push(original_path);
        let mut command = fixture.command();
        command
            .env("PATH", path)
            .env("CSA_REAL_GIT", &fake_git)
            .env("FAIL_PRODUCER", producer);

        let output = command
            .output_with_timeout()
            .expect("run producer failure fixture");
        assert!(!output.status.success(), "{producer}");
        assert_eq!(
            fixture.hook_count(),
            0,
            "{producer} failure must block before the hook"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("fingerprint state is unavailable"),
            "{producer}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn wrapper_fails_closed_on_oversized_fingerprint_inputs() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    for input_kind in ["worktree", "hook-size", "hook-count"] {
        let fixture = SandboxCommitFixture::new();
        match input_kind {
            "worktree" => {
                std::fs::write(
                    fixture.repo.join("fixture.txt"),
                    vec![b'x'; 9 * 1024 * 1024],
                )
                .expect("write oversized worktree diff");
            }
            "hook-size" => {
                std::fs::write(
                    fixture.repo.join(".git/hooks/large-helper"),
                    vec![b'x'; 2 * 1024 * 1024],
                )
                .expect("write oversized hook helper");
            }
            "hook-count" => {
                let helpers = fixture.repo.join(".git/hooks/lib");
                std::fs::create_dir(&helpers).expect("create hook helper dir");
                for index in 0..65 {
                    std::fs::write(helpers.join(format!("helper-{index}")), b"exit 1\n")
                        .expect("write hook helper");
                }
            }
            _ => unreachable!(),
        }

        let output = fixture.run();
        assert!(!output.status.success(), "{input_kind}");
        assert_eq!(
            fixture.hook_count(),
            0,
            "{input_kind} must block before the hook"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("fingerprint state is unavailable"),
            "{input_kind}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
