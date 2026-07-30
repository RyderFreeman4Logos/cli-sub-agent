use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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
fn read_process_start_time_ticks(pid: u32) -> u64 {
    let content = std::fs::read_to_string(format!("/proc/{pid}/stat")).expect("read /proc stat");
    let close_paren = content.rfind(')').expect("stat comm terminator");
    let mut parts = content[close_paren + 1..].split_whitespace();
    parts.next().expect("state");
    parts.next().expect("ppid");
    parts.next().expect("pgrp");
    for _ in 0..16 {
        parts.next().expect("intermediate stat field");
    }
    parts
        .next()
        .expect("starttime")
        .parse::<u64>()
        .expect("starttime parse")
}

#[cfg(target_os = "linux")]
#[test]
fn session_wait_liveness_failure_exits_nonzero_even_with_live_daemon() {
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

    let mut daemon = Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn live daemon stand-in");
    std::fs::write(
        session_dir.join("daemon.pid"),
        format!(
            "{} {}\n",
            daemon.id(),
            read_process_start_time_ticks(daemon.id())
        ),
    )
    .expect("write live daemon pid");

    let output = csa_cmd(tmp.path())
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
        .output()
        .expect("run csa session wait");

    daemon.kill().ok();
    daemon.wait().ok();

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
