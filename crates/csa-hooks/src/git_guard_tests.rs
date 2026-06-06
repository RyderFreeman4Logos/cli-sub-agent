#[cfg(unix)]
use crate::test_support::ENV_LOCK;

use crate::{git_wrapper_script, inject_git_guard_env};
use std::collections::HashMap;
#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
fn write_executable(path: &Path, contents: impl AsRef<[u8]>) {
    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(contents.as_ref()).unwrap();
    file.sync_all().unwrap();
    drop(file);

    for attempt in 0..5 {
        match std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)) {
            Ok(()) => return,
            Err(err) if err.raw_os_error() == Some(libc::ETXTBSY) && attempt < 4 => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(err) => panic!("failed to mark {} executable: {err}", path.display()),
        }
    }
}

#[cfg(unix)]
fn write_fake_worktree_git(path: &Path) {
    write_executable(
        path,
        r#"#!/usr/bin/env bash
set -euo pipefail
case "$*" in
  "rev-parse --is-inside-work-tree") exit 0 ;;
  "rev-parse --show-toplevel") printf '%s\n' "${FAKE_TOP}" ; exit 0 ;;
  "config --path --get core.hooksPath") exit 1 ;;
esac
if [ "${1:-}" = "rev-parse" ] && [ "${2:-}" = "--git-path" ]; then
  printf '%s\n' "${FAKE_TOP}/.git/${3}"
  exit 0
fi
printf '%s\n' "$*"
"#,
    );
}

#[test]
fn inject_git_guard_env_sets_real_git_and_path() {
    let mut env = HashMap::new();
    inject_git_guard_env(&mut env);

    assert!(env.contains_key("PATH"));
    assert!(
        env.get("PATH")
            .is_some_and(|value| value.contains("guards"))
    );
    assert!(env.contains_key("CSA_REAL_GIT"));
}

#[cfg(unix)]
#[test]
fn inject_git_guard_env_uses_session_bin_when_session_dir_is_present() {
    let temp = tempfile::tempdir().unwrap();
    let session_dir = temp.path().join("session");
    let mut env = HashMap::from([(
        "CSA_SESSION_DIR".to_string(),
        session_dir.display().to_string(),
    )]);

    inject_git_guard_env(&mut env);

    let expected_bin = session_dir.join("bin");
    let path = env.get("PATH").expect("PATH should be set");
    assert!(
        path.starts_with(expected_bin.to_string_lossy().as_ref()),
        "session bin should be prepended to PATH, got: {path}"
    );
    assert!(
        expected_bin.join("git").exists(),
        "git wrapper should be written into CSA_SESSION_DIR/bin"
    );
    let mode = std::fs::metadata(expected_bin.join("git"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o755, "git wrapper should be executable");
}

#[cfg(unix)]
#[test]
fn wrapper_script_parses_with_sh_n() {
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("git");
    write_executable(&wrapper, git_wrapper_script());

    let output = std::process::Command::new("sh")
        .arg("-n")
        .arg(&wrapper)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn wrapper_strips_no_verify_before_real_git() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("git");
    write_executable(&wrapper, git_wrapper_script());

    let fake_git = temp.path().join("real-git");
    write_executable(&fake_git, "#!/usr/bin/env bash\necho \"$@\"\n");

    let output = std::process::Command::new(&wrapper)
        .arg("commit")
        .arg("--no-verify")
        .arg("-m")
        .arg("test")
        .env("CSA_REAL_GIT", &fake_git)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "commit -m test"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim(),
        "CSA git-guard: stripped --no-verify from git commit (hook bypass forbidden)"
    );
}

#[cfg(unix)]
#[test]
fn wrapper_strips_short_no_verify_before_real_git() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("git");
    write_executable(&wrapper, git_wrapper_script());

    let fake_git = temp.path().join("real-git");
    write_executable(&fake_git, "#!/usr/bin/env bash\necho \"$@\"\n");

    let output = std::process::Command::new(&wrapper)
        .arg("commit")
        .arg("-n")
        .arg("-m")
        .arg("test")
        .env("CSA_REAL_GIT", &fake_git)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "commit -m test"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("stripped --no-verify"));
}

#[cfg(unix)]
#[test]
fn wrapper_strips_combined_short_no_verify_before_real_git() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("git");
    write_executable(&wrapper, git_wrapper_script());

    let fake_git = temp.path().join("real-git");
    write_executable(&fake_git, "#!/usr/bin/env bash\necho \"$@\"\n");

    for flag in ["-anm", "-nam"] {
        let output = std::process::Command::new(&wrapper)
            .arg("commit")
            .arg(flag)
            .arg("test")
            .env("CSA_REAL_GIT", &fake_git)
            .output()
            .unwrap();

        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "commit -am test"
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("stripped --no-verify"));
    }
}

#[cfg(unix)]
#[test]
fn wrapper_unsets_hook_bypass_env_for_any_git_command() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("git");
    write_executable(&wrapper, git_wrapper_script());

    let fake_git = temp.path().join("real-git");
    write_executable(
        &fake_git,
        r#"#!/usr/bin/env bash
set -euo pipefail
for name in LEFTHOOK LEFTHOOK_DISABLED HUSKY HUSKY_DISABLE SKIP_HOOKS SKIP_GIT_HOOKS PRE_COMMIT_ALLOW_NO_CONFIG LEFTHOOK_SKIP_PRE_COMMIT; do
  if env | grep -q "^${name}="; then
echo "${name} still set"
exit 3
  fi
done
echo "$@"
"#,
    );

    let output = std::process::Command::new(&wrapper)
        .arg("status")
        .env("CSA_REAL_GIT", &fake_git)
        .env("LEFTHOOK", "0")
        .env("LEFTHOOK_DISABLED", "1")
        .env("HUSKY", "0")
        .env("HUSKY_DISABLE", "1")
        .env("SKIP_HOOKS", "1")
        .env("SKIP_GIT_HOOKS", "1")
        .env("PRE_COMMIT_ALLOW_NO_CONFIG", "1")
        .env("LEFTHOOK_SKIP_PRE_COMMIT", "1")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "status");
}

#[cfg(unix)]
#[test]
fn wrapper_blocks_push_without_permission() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("git");
    write_executable(&wrapper, git_wrapper_script());

    let fake_git = temp.path().join("real-git");
    write_executable(&fake_git, "#!/usr/bin/env bash\necho \"$@\"\n");

    let output = std::process::Command::new(&wrapper)
        .arg("push")
        .env("CSA_REAL_GIT", &fake_git)
        .env_remove("CSA_GIT_PUSH_ALLOWED")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim(),
        "CSA git-guard: git push blocked for leaf-worker sessions."
    );
    assert!(String::from_utf8_lossy(&output.stdout).trim().is_empty());
}

#[cfg(unix)]
#[test]
fn wrapper_allows_push_with_permission() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("git");
    write_executable(&wrapper, git_wrapper_script());

    let fake_git = temp.path().join("real-git");
    write_executable(&fake_git, "#!/usr/bin/env bash\necho \"$@\"\n");

    let output = std::process::Command::new(&wrapper)
        .arg("push")
        .env("CSA_REAL_GIT", &fake_git)
        .env("CSA_GIT_PUSH_ALLOWED", "true")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "push");
}

#[cfg(unix)]
#[test]
fn wrapper_allows_pre_push_only_lefthook_config_without_pre_commit() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("git");
    write_executable(&wrapper, git_wrapper_script());

    let repo = temp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git/hooks")).unwrap();
    std::fs::write(
        repo.join("lefthook.yml"),
        "pre-push:\n  commands:\n    review:\n      run: echo ok\n",
    )
    .unwrap();
    write_executable(
        repo.join(".git/hooks/pre-push").as_path(),
        "#!/usr/bin/env bash\n",
    );

    let fake_git = temp.path().join("real-git");
    write_fake_worktree_git(&fake_git);

    let output = std::process::Command::new(&wrapper)
        .arg("commit")
        .arg("-m")
        .arg("test")
        .current_dir(&repo)
        .env("CSA_REAL_GIT", &fake_git)
        .env("FAKE_TOP", &repo)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "commit -m test"
    );
}

#[cfg(unix)]
#[test]
fn wrapper_blocks_missing_defined_pre_push_lefthook_hook() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("git");
    write_executable(&wrapper, git_wrapper_script());

    let repo = temp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git/hooks")).unwrap();
    std::fs::write(repo.join("lefthook.yml"), "pre-push:\n").unwrap();

    let fake_git = temp.path().join("real-git");
    write_fake_worktree_git(&fake_git);

    let output = std::process::Command::new(&wrapper)
        .arg("commit")
        .arg("-m")
        .arg("test")
        .current_dir(&repo)
        .env("CSA_REAL_GIT", &fake_git)
        .env("FAKE_TOP", &repo)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("pre-push"), "{stderr}");
    assert!(stderr.contains("lefthook install"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn wrapper_requires_pre_commit_when_lefthook_config_defines_it() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("git");
    write_executable(&wrapper, git_wrapper_script());

    let repo = temp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git/hooks")).unwrap();
    std::fs::write(repo.join("lefthook.yml"), "pre-commit:\npre-push:\n").unwrap();
    write_executable(
        repo.join(".git/hooks/pre-push").as_path(),
        "#!/usr/bin/env bash\n",
    );

    let fake_git = temp.path().join("real-git");
    write_fake_worktree_git(&fake_git);

    let output = std::process::Command::new(&wrapper)
        .arg("commit")
        .arg("-m")
        .arg("test")
        .current_dir(&repo)
        .env("CSA_REAL_GIT", &fake_git)
        .env("FAKE_TOP", &repo)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("pre-commit"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn wrapper_allows_long_commit_options_containing_n() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("git");
    write_executable(&wrapper, git_wrapper_script());

    let fake_git = temp.path().join("real-git");
    write_executable(&fake_git, "#!/usr/bin/env bash\necho \"$@\"\n");

    let output = std::process::Command::new(&wrapper)
        .arg("commit")
        .arg("--amend")
        .env("CSA_REAL_GIT", &fake_git)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "commit --amend"
    );
}

#[cfg(unix)]
#[test]
fn wrapper_allows_markdown_bullet_after_long_message_option() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("git");
    write_executable(&wrapper, git_wrapper_script());

    let fake_git = temp.path().join("real-git");
    write_executable(&fake_git, "#!/usr/bin/env bash\necho \"$@\"\n");

    // Commit message body intentionally starts with "- " (markdown bullet),
    // which is NOT a separate flag — bind to a variable so clippy does not
    // misinterpret the leading dash as a split-args candidate.
    let body_msg = "- **Design Intent**: count failed reads as skipped";
    let output = std::process::Command::new(&wrapper)
        .arg("commit")
        .arg("--message")
        .arg("fix(batch): count read failures as skipped")
        .arg("--message")
        .arg(body_msg)
        .env("CSA_REAL_GIT", &fake_git)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "commit --message fix(batch): count read failures as skipped --message - **Design Intent**: count failed reads as skipped"
    );
}

#[cfg(unix)]
#[test]
fn wrapper_allows_leading_dash_after_short_message_option() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("git");
    write_executable(&wrapper, git_wrapper_script());

    let fake_git = temp.path().join("real-git");
    write_executable(&fake_git, "#!/usr/bin/env bash\necho \"$@\"\n");

    // Message intentionally starts with "- " (markdown bullet); bind to a
    // variable so clippy does not flag suspicious_command_arg_space.
    let msg = "- leading dash is message text";
    let output = std::process::Command::new(&wrapper)
        .arg("commit")
        .arg("-m")
        .arg(msg)
        .env("CSA_REAL_GIT", &fake_git)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "commit -m - leading dash is message text"
    );
}

#[cfg(unix)]
#[test]
fn wrapper_allows_leading_dash_after_file_option() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("git");
    write_executable(&wrapper, git_wrapper_script());

    let fake_git = temp.path().join("real-git");
    write_executable(&fake_git, "#!/usr/bin/env bash\necho \"$@\"\n");

    let output = std::process::Command::new(&wrapper)
        .arg("commit")
        .arg("-F")
        .arg("-")
        .env("CSA_REAL_GIT", &fake_git)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "commit -F -"
    );
}

#[cfg(unix)]
#[test]
fn wrapper_forwards_non_commit_commands() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let temp = tempfile::tempdir().unwrap();
    let wrapper = temp.path().join("git");
    write_executable(&wrapper, git_wrapper_script());

    let fake_git = temp.path().join("real-git");
    write_executable(&fake_git, "#!/usr/bin/env bash\necho \"$@\"\n");

    for subcommand in ["status", "add", "diff"] {
        let output = std::process::Command::new(&wrapper)
            .arg(subcommand)
            .env("CSA_REAL_GIT", &fake_git)
            .output()
            .unwrap();

        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), subcommand);
    }
}
