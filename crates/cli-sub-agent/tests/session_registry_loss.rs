use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn csa_cmd(tmp: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_csa"));
    scrub_inherited_csa_env(&mut cmd);
    cmd.env("HOME", tmp)
        .env("XDG_STATE_HOME", tmp.join(".local/state"))
        .env("XDG_CONFIG_HOME", tmp.join(".config"))
        .env("HERMES_MODEL_PROVIDER", "openai")
        .env("TOKIO_WORKER_THREADS", "1");
    cmd
}

fn scrub_inherited_csa_env(cmd: &mut Command) {
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("CSA_") {
            cmd.env_remove(key);
        }
    }
}

fn global_config_path(tmp: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        tmp.join("Library/Application Support/cli-sub-agent/config.toml")
    } else {
        tmp.join(".config/cli-sub-agent/config.toml")
    }
}

fn session_dir_for(tmp: &Path, project: &Path, session_id: &str) -> PathBuf {
    let canonical_project = project.canonicalize().expect("canonical project path");
    let storage_key = canonical_project
        .to_string_lossy()
        .trim_start_matches('/')
        .replace('/', std::path::MAIN_SEPARATOR_STR);
    state_root_for(tmp)
        .join(storage_key)
        .join("sessions")
        .join(session_id)
}

fn state_root_for(tmp: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        tmp.join("Library/Application Support/cli-sub-agent")
    } else {
        tmp.join(".local/state/cli-sub-agent")
    }
}

fn synthetic_registered_dir(tmp: &Path, project: &Path, session_id: &str) -> PathBuf {
    let session_dir = session_dir_for(tmp, project, session_id);
    std::fs::create_dir_all(session_dir.join("input")).expect("create synthetic input dir");
    std::fs::create_dir_all(session_dir.join("output")).expect("create synthetic output dir");
    session_dir
}

#[cfg(unix)]
fn set_mtime_seconds_ago(path: &Path, seconds_ago: u64) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before unix epoch");
    let target = now.saturating_sub(std::time::Duration::from_secs(seconds_ago));
    let times = [
        libc::timespec {
            tv_sec: target.as_secs() as libc::time_t,
            tv_nsec: target.subsec_nanos() as libc::c_long,
        },
        libc::timespec {
            tv_sec: target.as_secs() as libc::time_t,
            tv_nsec: target.subsec_nanos() as libc::c_long,
        },
    ];
    let c_path = CString::new(path.as_os_str().as_bytes()).expect("path contains NUL");
    // SAFETY: `c_path` is a NUL-terminated path and `times` lives until the call returns.
    let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) };
    assert_eq!(rc, 0, "utimensat failed for {}", path.display());
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn write_pre_exec_result(session_dir: &Path, summary: &str) {
    std::fs::write(
        session_dir.join("result.toml"),
        format!(
            r#"status = "failure"
exit_code = 1
summary = {summary:?}
tool = "codex"
started_at = "2026-04-27T00:00:00Z"
completed_at = "2026-04-27T00:00:01Z"
gate_timeout = false
"#
        ),
    )
    .expect("write result.toml");
}

fn assert_session_result_summary_prefers_result_toml_over_registry_loss(corrupt_state: bool) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).expect("create project");
    let session_id = csa_session::new_session_id();
    let session_dir = synthetic_registered_dir(tmp.path(), &project, &session_id);
    if corrupt_state {
        std::fs::write(
            session_dir.join("state.toml"),
            "this is not valid toml {{{\n",
        )
        .expect("write corrupt state");
    }
    write_pre_exec_result(&session_dir, "pre-exec: host memory admission denied");

    let output = csa_cmd(tmp.path())
        .args([
            "session",
            "result",
            "--session",
            &session_id,
            "--summary",
            "--cd",
            project.to_str().expect("utf-8 project path"),
        ])
        .output()
        .expect("run csa session result --summary");

    assert!(output.status.success(), "{}", output_text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("pre-exec: host memory admission denied"),
        "bounded result.toml summary should be printed first, got stdout={stdout} stderr={stderr}"
    );
    assert!(
        !stderr.contains("session registry lookup failed"),
        "registry-loss diagnostics must not suppress a bounded result.toml summary: {stderr}"
    );
}

fn assert_session_result_prefers_result_toml_over_registry_loss(corrupt_state: bool) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).expect("create project");
    let session_id = csa_session::new_session_id();
    let session_dir = synthetic_registered_dir(tmp.path(), &project, &session_id);
    if corrupt_state {
        std::fs::write(
            session_dir.join("state.toml"),
            "this is not valid toml {{{\n",
        )
        .expect("write corrupt state");
    }
    write_pre_exec_result(&session_dir, "pre-exec: host memory admission denied");

    let output = csa_cmd(tmp.path())
        .args([
            "session",
            "result",
            "--session",
            &session_id,
            "--cd",
            project.to_str().expect("utf-8 project path"),
        ])
        .output()
        .expect("run csa session result");

    assert!(output.status.success(), "{}", output_text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains(&format!("Session: {session_id}")),
        "{stdout}"
    );
    assert!(stdout.contains("Status:  failure"), "{stdout}");
    assert!(stdout.contains("Exit:    1"), "{stdout}");
    assert!(
        stdout.contains("Summary: pre-exec: host memory admission denied"),
        "{stdout}"
    );
    assert!(
        !stderr.contains("session registry lookup failed"),
        "registry-loss diagnostics must not suppress result.toml: {stderr}"
    );
}

#[test]
fn session_result_summary_prefers_result_toml_when_state_is_missing() {
    assert_session_result_summary_prefers_result_toml_over_registry_loss(false);
}

#[test]
fn session_result_summary_prefers_result_toml_when_state_is_corrupt() {
    assert_session_result_summary_prefers_result_toml_over_registry_loss(true);
}

#[test]
fn session_result_prefers_result_toml_when_state_is_missing() {
    assert_session_result_prefers_result_toml_over_registry_loss(false);
}

#[test]
fn session_result_prefers_result_toml_when_state_is_corrupt() {
    assert_session_result_prefers_result_toml_over_registry_loss(true);
}

#[test]
fn session_result_classifies_missing_state_as_registry_loss() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).expect("create project");
    let session_id = csa_session::new_session_id();
    let session_dir = synthetic_registered_dir(tmp.path(), &project, &session_id);
    std::fs::write(session_dir.join("stdout.log"), "captured progress\n")
        .expect("write synthetic stdout log");

    let output = csa_cmd(tmp.path())
        .args([
            "session",
            "result",
            "--session",
            &session_id,
            "--cd",
            project.to_str().expect("utf-8 project path"),
        ])
        .output()
        .expect("run csa session result");

    assert!(output.status.success(), "{}", output_text(&output));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("session registry lookup failed"),
        "{stderr}"
    );
    assert!(stderr.contains("CSA infrastructure"), "{stderr}");
    assert!(stderr.contains("not a product-code failure"), "{stderr}");
    assert!(stderr.contains("git status --short"), "{stderr}");
    assert!(stderr.contains("git diff --staged"), "{stderr}");
    assert!(
        stderr.contains("Do not manually read session directories or transcripts"),
        "{stderr}"
    );
    assert!(!stderr.contains("csa session logs --session"), "{stderr}");
    assert!(!stderr.contains("csa session list"), "{stderr}");
}

#[test]
fn session_result_lookup_miss_uses_exact_id_registry_loss_without_list_hint() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).expect("create project");
    let session_id = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

    let output = csa_cmd(tmp.path())
        .args([
            "session",
            "result",
            "--session",
            session_id,
            "--cd",
            project.to_str().expect("utf-8 project path"),
        ])
        .output()
        .expect("run csa session result");

    assert_eq!(output.status.code(), Some(1), "{}", output_text(&output));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("session registry lookup failed"),
        "{stderr}"
    );
    assert!(stderr.contains(session_id), "{stderr}");
    assert!(stderr.contains("CSA:SESSION_STARTED"), "{stderr}");
    assert!(stderr.contains("git status --short"), "{stderr}");
    assert!(!stderr.contains("csa session list"), "{stderr}");
}

#[test]
fn session_wait_lookup_miss_reports_structured_gc_pruned_registry_loss() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).expect("create project");
    let session_id = "01ARZ3NDEKTSV4RRFFQ69G5FBG";

    let output = csa_cmd(tmp.path())
        .args([
            "session",
            "wait",
            "--session",
            session_id,
            "--cd",
            project.to_str().expect("utf-8 project path"),
        ])
        .output()
        .expect("run csa session wait");

    assert_eq!(output.status.code(), Some(1), "{}", output_text(&output));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("CSA:SESSION_REGISTRY_LOSS"), "{stderr}");
    assert!(stderr.contains("reason=lookup_miss_or_gc"), "{stderr}");
    assert!(
        stderr.contains("whole session") || stderr.contains("whole-session"),
        "{stderr}"
    );
    assert!(
        stderr.contains("result.toml was inside the removed session directory"),
        "{stderr}"
    );
    assert!(!stderr.contains("csa session list"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn session_wait_classifies_corrupt_state_as_registry_loss() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_path = global_config_path(tmp.path());
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("create config dir");
    std::fs::write(&config_path, "[kv_cache.provider_ttls]\nopenai = 1\n")
        .expect("write short provider wait config");

    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).expect("create project");
    let session_id = csa_session::new_session_id();
    let session_dir = synthetic_registered_dir(tmp.path(), &project, &session_id);
    let state_path = session_dir.join("state.toml");
    std::fs::write(&state_path, "this is not valid toml {{{\n").expect("write corrupt state");
    set_mtime_seconds_ago(&state_path, 5);

    let output = csa_cmd(tmp.path())
        .args([
            "session",
            "wait",
            "--session",
            &session_id,
            "--cd",
            project.to_str().expect("utf-8 project path"),
        ])
        .output()
        .expect("run csa session wait");

    assert_eq!(output.status.code(), Some(1), "{}", output_text(&output));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("session registry lookup failed"),
        "{stderr}"
    );
    assert!(stderr.contains("corrupt state.toml"), "{stderr}");
    assert!(stderr.contains("CSA infrastructure"), "{stderr}");
    assert!(stderr.contains("not a product-code failure"), "{stderr}");
    assert!(!stderr.contains("csa session list"), "{stderr}");
}
