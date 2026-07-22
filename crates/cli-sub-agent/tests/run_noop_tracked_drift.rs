use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn csa_cmd(home: &Path) -> Command {
    let cargo_home = home.join(".cargo");
    let rustup_home = home.join(".rustup");
    std::fs::create_dir_all(&cargo_home).expect("create isolated cargo home");
    std::fs::create_dir_all(&rustup_home).expect("create isolated rustup home");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_csa"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("CSA_") {
            cmd.env_remove(key);
        }
    }
    cmd.env("HOME", home)
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("CARGO_HOME", cargo_home)
        .env("RUSTUP_HOME", rustup_home)
        .env("TOKIO_WORKER_THREADS", "1")
        .env_remove("CI");
    cmd
}

fn run_git(project_root: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .args(args)
        .current_dir(project_root)
        .output()
        .expect("git should run")
}

fn require_git(project_root: &Path, args: &[&str]) {
    let output = run_git(project_root, args);
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_tracked_project(project_root: &Path) {
    std::fs::create_dir_all(project_root.join(".csa")).expect("create project config dir");
    std::fs::write(project_root.join("README.md"), "# test project\n").expect("write readme");
    std::fs::write(project_root.join("lefthook.yml"), "pre-commit:\n")
        .expect("write tracked lefthook config");
    std::fs::write(
        project_root.join(".csa/config.toml"),
        r#"schema_version = 1

[resources]
min_free_memory_mb = 0
memory_max_mb = 9000
soft_limit_percent = 100

[filesystem_sandbox]
enforcement_mode = "off"

[tools.codex]
enabled = true
transport = "cli"
default_model = "gpt-5.4-mini"

[run.post_exec_gate]
enabled = false
"#,
    )
    .expect("write project config");

    let output = Command::new("git")
        .args(["-c", "init.defaultBranch=main", "init"])
        .current_dir(project_root)
        .output()
        .expect("git init should run");
    assert!(
        output.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    require_git(project_root, &["config", "user.email", "test@example.com"]);
    require_git(project_root, &["config", "user.name", "Test User"]);
    require_git(project_root, &["add", "."]);
    require_git(project_root, &["commit", "-m", "initial"]);
    require_git(project_root, &["checkout", "-b", "feat/noop-drift"]);
}

#[cfg(unix)]
fn install_noop_codex(bin_dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    std::fs::create_dir_all(bin_dir).expect("create fake tool directory");
    let codex = bin_dir.join("codex");
    std::fs::write(
        &codex,
        r#"#!/bin/sh
printf '%s\n' \
  '{"type":"thread.started","thread_id":"no-op-tracked-drift"}' \
  '{"type":"item.completed","item":{"type":"agent_message","text":"done"}}' \
  '{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}'
"#,
    )
    .expect("write fake codex");
    let mut permissions = std::fs::metadata(&codex)
        .expect("fake codex metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&codex, permissions).expect("make fake codex executable");
    bin_dir.to_path_buf()
}

#[cfg(unix)]
fn prepend_path(bin_dir: &Path) -> OsString {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin_dir.to_path_buf()];
    paths.extend(std::env::split_paths(&current));
    std::env::join_paths(paths).expect("join PATH")
}

fn global_config_path(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/cli-sub-agent/config.toml")
    } else {
        home.join(".config/cli-sub-agent/config.toml")
    }
}

#[cfg(unix)]
#[test]
fn noop_writer_session_leaves_zero_tracked_lefthook_drift() {
    let home = tempfile::tempdir().expect("create temporary home");
    let project = home.path().join("project");
    init_tracked_project(&project);

    let config_path = global_config_path(home.path());
    std::fs::create_dir_all(config_path.parent().expect("global config parent"))
        .expect("create global config dir");
    std::fs::write(config_path, "[hooks]\nauto_setup_review_gate = true\n")
        .expect("enable launcher review-gate path");

    let fake_bin = install_noop_codex(&home.path().join("bin"));
    let output = csa_cmd(home.path())
        .current_dir(&project)
        .env("PATH", prepend_path(&fake_bin))
        .args([
            "run",
            "--no-daemon",
            "--sa-mode",
            "true",
            "--tool",
            "codex",
            "--min-free-memory-mb",
            "0",
            "reply without tool calls",
        ])
        .output()
        .expect("run no-op writer session");

    assert!(
        String::from_utf8_lossy(&output.stdout).contains("no-op-tracked-drift"),
        "writer provider must start before the no-op result: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(project.join("lefthook.yml")).expect("read lefthook config"),
        "pre-commit:\n",
        "no-op writer session must preserve the tracked lefthook config"
    );
    assert_eq!(
        String::from_utf8_lossy(
            &run_git(&project, &["status", "--porcelain", "--untracked-files=no"]).stdout
        ),
        "",
        "no-op writer session must leave zero tracked worktree drift"
    );
}
