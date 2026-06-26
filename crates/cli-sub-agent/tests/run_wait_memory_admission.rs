use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn csa_cmd(tmp: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_csa"));
    scrub_inherited_csa_env(&mut cmd);
    cmd.env("HOME", tmp)
        .env("XDG_STATE_HOME", tmp.join(".local/state"))
        .env("XDG_CONFIG_HOME", tmp.join(".config"))
        .env("TOKIO_WORKER_THREADS", "1")
        .env("CSA_DAEMON_INDEPENDENT_SCOPE", "0");
    cmd
}

fn scrub_inherited_csa_env(cmd: &mut Command) {
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("CSA_") {
            cmd.env_remove(key);
        }
    }
}

fn write_fake_codex_bin(tmp: &Path) -> PathBuf {
    let bin_dir = tmp.join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create fake bin dir");
    let codex = bin_dir.join("codex");
    std::fs::write(
        &codex,
        "#!/bin/sh\necho fake codex should not run >&2\nexit 77\n",
    )
    .expect("write fake codex");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&codex)
            .expect("fake codex metadata")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&codex, perms).expect("chmod fake codex");
    }
    bin_dir
}

fn prepend_path(bin_dir: &Path) -> String {
    let existing = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = std::env::split_paths(&existing).collect::<Vec<_>>();
    paths.insert(0, bin_dir.to_path_buf());
    std::env::join_paths(paths)
        .expect("join PATH")
        .to_string_lossy()
        .into_owned()
}

fn stdout_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn run_wait_host_memory_denial_fails_before_session_started_marker() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("repo");
    std::fs::create_dir_all(project.join(".csa")).expect("create project config dir");
    std::fs::write(
        project.join(".csa/config.toml"),
        "[resources]\nslot_wait_timeout_seconds = 1\n",
    )
    .expect("write project config");
    let fake_bin = write_fake_codex_bin(tmp.path());

    let output = csa_cmd(tmp.path())
        .current_dir(&project)
        .env("PATH", prepend_path(&fake_bin))
        .args([
            "run",
            "--sa-mode",
            "false",
            "--wait",
            "--tool",
            "codex",
            "--memory-max-mb",
            "999999999999",
            "--min-free-memory-mb",
            "0",
            "memory admission regression",
        ])
        .output()
        .expect("run csa");

    let stdout = stdout_text(&output);
    let stderr = stderr_text(&output);

    assert!(
        !output.status.success(),
        "run should fail host-memory admission; stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        !stdout.contains("CSA:SESSION_STARTED"),
        "--wait must not emit SESSION_STARTED marker for pre-exec host-memory denial; stdout={stdout:?}"
    );
    assert!(
        !stderr.contains("CSA:SESSION_STARTED"),
        "--wait must not emit SESSION_STARTED marker for pre-exec host-memory denial; stderr={stderr:?}"
    );
    assert!(
        stdout.trim().is_empty(),
        "--wait host-memory denial must not print a waitable session id; stdout={stdout:?}"
    );
    assert!(
        stderr.contains("host memory admission"),
        "stderr should surface the host-memory denial; stderr={stderr:?}"
    );
    assert!(
        stderr.contains("no `csa session wait` is needed"),
        "stderr should tell the caller no session wait is needed; stderr={stderr:?}"
    );
}
