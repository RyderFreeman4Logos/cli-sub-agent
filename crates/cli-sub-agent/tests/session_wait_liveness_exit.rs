use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use chrono::Utc;
use csa_session::{MetaSessionState, SessionPhase};

fn csa_cmd(tmp: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_csa"));
    scrub_inherited_csa_env(&mut cmd);
    cmd.env("HOME", tmp)
        .env("XDG_STATE_HOME", tmp.join(".local/state"))
        .env("XDG_CONFIG_HOME", tmp.join(".config"))
        .env("HERMES_MODEL_PROVIDER", "custom:z2")
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

fn session_root_for(tmp: &Path, project: &Path) -> PathBuf {
    let canonical_project = project.canonicalize().expect("canonical project path");
    let storage_key = canonical_project
        .to_string_lossy()
        .trim_start_matches('/')
        .replace('/', std::path::MAIN_SEPARATOR_STR);
    let state_root = if cfg!(target_os = "macos") {
        tmp.join("Library/Application Support/cli-sub-agent")
    } else {
        tmp.join(".local/state/cli-sub-agent")
    };
    state_root.join(storage_key)
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[cfg(target_os = "linux")]
fn backdate_tree(path: &Path, seconds_ago: u64) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    if path.is_dir() {
        for entry in std::fs::read_dir(path).expect("read session tree") {
            backdate_tree(&entry.expect("session tree entry").path(), seconds_ago);
        }
    }
    let target = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .saturating_sub(Duration::from_secs(seconds_ago));
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
    let path = CString::new(path.as_os_str().as_bytes()).expect("path contains no NUL");
    // SAFETY: `path` is NUL-terminated and `times` lives for the system call.
    assert_eq!(
        unsafe { libc::utimensat(libc::AT_FDCWD, path.as_ptr(), times.as_ptr(), 0) },
        0,
        "backdate {}",
        path.to_string_lossy()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn session_wait_liveness_failure_exits_nonzero_without_live_signal() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(project.join(".csa")).expect("create project config dir");
    std::fs::write(
        project.join(".csa/config.toml"),
        "schema_version = 1\n[resources]\nliveness_dead_seconds = 1\n",
    )
    .expect("write project liveness config");

    let global_config = global_config_path(tmp.path());
    std::fs::create_dir_all(global_config.parent().expect("global config parent"))
        .expect("create global config dir");
    std::fs::write(
        global_config,
        "[kv_cache.provider_ttls]\n\"custom:z2\" = 30\n",
    )
    .expect("write provider wait config");

    let session_root = session_root_for(tmp.path(), &project);
    let stale_at = Utc::now() - chrono::Duration::seconds(2);
    let session = MetaSessionState {
        meta_session_id: csa_session::new_session_id(),
        description: Some("stale live daemon CLI exit regression".to_string()),
        project_path: project
            .canonicalize()
            .expect("canonical project path")
            .to_string_lossy()
            .into_owned(),
        created_at: stale_at,
        last_accessed: stale_at,
        phase: SessionPhase::Active,
        ..Default::default()
    };
    let session_dir = session_root.join("sessions").join(&session.meta_session_id);
    std::fs::create_dir_all(session_dir.join("input")).expect("create session input dir");
    std::fs::create_dir_all(session_dir.join("output")).expect("create session output dir");
    csa_session::save_session_in(&session_root, &session).expect("save stale active session");
    backdate_tree(&session_dir, 31);

    let mut wait = csa_cmd(tmp.path())
        .current_dir(&project)
        .args([
            "session",
            "wait",
            "--session",
            &session.meta_session_id,
            "--model-provider",
            "custom:z2",
            "--cd",
            project.to_str().expect("utf-8 project path"),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn csa session wait");

    let wait_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match wait.try_wait().expect("poll csa session wait") {
            Some(_) => break,
            None if Instant::now() < wait_deadline => std::thread::sleep(Duration::from_millis(10)),
            None => {
                wait.kill().ok();
                wait.wait().ok();
                panic!("session wait did not fail before the test watchdog expired");
            }
        }
    }
    let output = wait
        .wait_with_output()
        .expect("collect csa session wait output");

    assert_eq!(
        output.status.code(),
        Some(1),
        "stale liveness is a failure, never a successful wait cap: {}",
        output_text(&output)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("liveness failure"),
        "{}",
        output_text(&output)
    );
    assert!(
        stderr.contains("liveness_dead_seconds=1s"),
        "{}",
        output_text(&output)
    );
}
