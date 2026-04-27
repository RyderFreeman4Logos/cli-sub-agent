use std::path::Path;
use std::process::Command;

use serial_test::serial;

const PRE_EXEC_SUMMARY: &str = "pre-exec: Direct --tool is blocked when tiers are configured.";

struct EnvVarGuard {
    key: &'static str,
    original: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: this e2e test is serialized while mutating process-wide env.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: this e2e test is serialized while restoring process-wide env.
        unsafe {
            match self.original.as_deref() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

fn csa_cmd(tmp: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_csa"));
    cmd.env("HOME", tmp)
        .env("XDG_STATE_HOME", tmp.join(".local/state"))
        .env("XDG_CONFIG_HOME", tmp.join(".config"))
        .env("TOKIO_WORKER_THREADS", "1");
    cmd
}

fn write_pre_exec_failure_result(session_dir: &Path) {
    std::fs::create_dir_all(session_dir.join("output")).expect("create empty output dir");
    std::fs::write(session_dir.join("stdout.log"), "").expect("write empty stdout log");
    std::fs::write(
        session_dir.join("result.toml"),
        format!(
            r#"status = "failure"
exit_code = 1
summary = "{PRE_EXEC_SUMMARY}"
tool = "codex"
started_at = "2026-04-27T00:00:00Z"
completed_at = "2026-04-27T00:00:01Z"
"#
        ),
    )
    .expect("write result.toml");
}

fn create_empty_output_failure_session(tmp: &Path, name: &str) -> (std::path::PathBuf, String) {
    let state_home = tmp.join(".local/state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", tmp);
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);

    let project = tmp.join(name);
    std::fs::create_dir_all(&project).expect("create project");
    let session =
        csa_session::create_session(&project, Some(name), None, Some("codex")).expect("session");
    let session_id = session.meta_session_id;
    let session_dir = csa_session::get_session_dir(&project, &session_id).expect("session dir");
    write_pre_exec_failure_result(&session_dir);

    (project, session_id)
}

fn assert_subcommand_surfaces_summary(subcommand: &str, session_name: &str) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (project, session_id) = create_empty_output_failure_session(tmp.path(), session_name);

    let output = csa_cmd(tmp.path())
        .args([
            "session",
            subcommand,
            "--session",
            &session_id,
            "--cd",
            project.to_str().expect("project path utf8"),
        ])
        .current_dir(&project)
        .output()
        .expect("run csa session command");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(PRE_EXEC_SUMMARY),
        "stderr should surface result.toml summary, got: {stderr}"
    );
}

#[test]
#[serial]
fn session_wait_surfaces_result_summary_when_failure_output_is_empty() {
    assert_subcommand_surfaces_summary("wait", "wait-empty-output-failure");
}

#[test]
#[serial]
fn session_attach_surfaces_result_summary_when_failure_output_is_empty() {
    assert_subcommand_surfaces_summary("attach", "attach-empty-output-failure");
}
