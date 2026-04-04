use super::*;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[cfg(unix)]
#[test]
fn ensure_terminal_result_for_dead_active_session_is_noop_when_result_exists() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(project, Some("already-has-result"), None, None).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    backdate_tree(&session_dir, 120);

    let existing = SessionResult {
        summary: "existing result".to_string(),
        ..make_result("failure", 9)
    };
    save_result(project, &session_id, &existing).unwrap();

    let reconciled =
        ensure_terminal_result_for_dead_active_session(project, &session_id, "session is-alive")
            .unwrap();
    assert!(!reconciled);

    let result = load_result(project, &session_id).unwrap().unwrap();
    assert_eq!(result.summary, "existing result");
    assert_eq!(result.exit_code, 9);
}

#[cfg(unix)]
#[test]
fn session_to_json_reconciles_orphaned_active_session_status() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let _daemon_project_guard = EnvVarGuard::set("CSA_DAEMON_PROJECT_ROOT", "");
    let _daemon_dir_guard = EnvVarGuard::set("CSA_DAEMON_SESSION_DIR", "");
    let project = td.path();

    let created = create_session(project, Some("json-reconcile"), None, None).unwrap();
    let session_id = created.meta_session_id.clone();
    let session_dir = get_session_dir(project, &session_id).unwrap();
    backdate_tree(&session_dir, 120);

    let session = load_session(project, &session_id).unwrap();
    let value = session_to_json(&session);
    assert_eq!(value.get("status").and_then(|v| v.as_str()), Some("Failed"));

    let persisted = load_result(project, &session_id).unwrap();
    assert!(
        persisted.is_some(),
        "status resolution should persist fallback result"
    );
}

#[cfg(unix)]
#[test]
fn ensure_terminal_result_for_dead_active_session_is_noop_for_non_active_phase() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let created = create_session(project, Some("available-session"), None, None).unwrap();
    let session_id = created.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();

    let mut session = load_session(project, &session_id).unwrap();
    session.phase = SessionPhase::Available;
    save_session(&session).unwrap();

    backdate_tree(&session_dir, 120);

    let reconciled =
        ensure_terminal_result_for_dead_active_session(project, &session_id, "session list")
            .unwrap();
    assert!(!reconciled);
    assert!(load_result(project, &session_id).unwrap().is_none());
}

#[cfg(unix)]
#[test]
fn ensure_terminal_result_for_dead_active_session_is_noop_when_alive() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let created = create_session(project, Some("alive-session"), None, None).unwrap();
    let session_id = created.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    let locks_dir = session_dir.join("locks");
    std::fs::create_dir_all(&locks_dir).unwrap();
    std::fs::write(
        locks_dir.join("codex.lock"),
        format!(r#"{{"pid": {}}}"#, std::process::id()),
    )
    .unwrap();

    let reconciled =
        ensure_terminal_result_for_dead_active_session(project, &session_id, "session list")
            .unwrap();
    assert!(!reconciled);
    assert!(load_result(project, &session_id).unwrap().is_none());
}

#[cfg(unix)]
#[test]
fn ensure_terminal_result_for_dead_active_session_persists_into_legacy_session_dir() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let created = create_session(project, Some("legacy-session"), None, None).unwrap();
    let session_id = created.meta_session_id;
    let primary_root = get_session_root(project).unwrap();
    let primary_session_dir = primary_root.join("sessions").join(&session_id);
    let legacy_sessions_dir = super::super::legacy_sessions_dir_from_primary_root(&primary_root)
        .expect("legacy session dir should resolve");
    let legacy_session_dir = legacy_sessions_dir.join(&session_id);
    std::fs::create_dir_all(&legacy_sessions_dir).unwrap();
    std::fs::rename(&primary_session_dir, &legacy_session_dir).unwrap();

    backdate_tree(&legacy_session_dir, 120);

    let reconciled =
        ensure_terminal_result_for_dead_active_session(project, &session_id, "session list")
            .unwrap();
    assert!(reconciled);
    assert!(
        legacy_session_dir
            .join(csa_session::result::RESULT_FILE_NAME)
            .is_file(),
        "legacy session dir should receive synthetic result"
    );
    assert!(load_result(project, &session_id).unwrap().is_some());

    delete_session(project, &session_id).unwrap();
}

#[cfg(unix)]
#[test]
fn handle_session_is_alive_reconciles_orphaned_active_session() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let created = create_session(project, Some("is-alive-reconcile"), None, None).unwrap();
    let session_id = created.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    backdate_tree(&session_dir, 120);

    let alive = handle_session_is_alive(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
    )
    .unwrap();
    assert!(!alive);

    let result = load_result(project, &session_id).unwrap();
    assert!(
        result.is_some(),
        "is-alive should reconcile missing terminal result"
    );
}

#[cfg(unix)]
#[test]
fn handle_session_is_alive_accepts_symlinked_project_path() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let alias = td.path().join("project-alias");
    std::os::unix::fs::symlink(&project, &alias).unwrap();

    let created = create_session(&alias, Some("is-alive-symlink"), None, None).unwrap();
    let session_id = created.meta_session_id;
    let session_dir = get_session_dir(&alias, &session_id).unwrap();
    backdate_tree(&session_dir, 120);

    let alive = handle_session_is_alive(
        session_id.clone(),
        Some(alias.to_string_lossy().into_owned()),
    )
    .unwrap();
    assert!(!alive);

    let canonical_project = project.canonicalize().unwrap();
    let result = load_result(&canonical_project, &session_id).unwrap();
    assert!(
        result.is_some(),
        "is-alive should find sessions created through symlinked project paths"
    );
}

#[cfg(unix)]
#[test]
fn handle_session_wait_reconciles_dead_lock_only_active_session() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(project, Some("wait-reconcile"), None, Some("codex")).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    let locks_dir = session_dir.join("locks");
    std::fs::create_dir_all(&locks_dir).unwrap();
    std::fs::write(
        locks_dir.join("codex.lock"),
        r#"{"pid": 999999999, "tool_name": "codex"}"#,
    )
    .unwrap();
    backdate_tree(&session_dir, 120);

    let exit_code = handle_session_wait(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        1,
    )
    .unwrap();
    assert_eq!(exit_code, 1);

    let result = load_result(project, &session_id).unwrap().unwrap();
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
    assert!(
        result.summary.contains("session wait"),
        "wait-triggered reconciliation should persist the synthetic failure summary"
    );

    let persisted = load_session(project, &session_id).unwrap();
    assert_eq!(
        persisted.termination_reason.as_deref(),
        Some("orphaned_process")
    );
}

#[cfg(unix)]
#[test]
fn ensure_terminal_result_for_dead_active_session_persists_synthetic_failure() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(project, Some("orphan"), None, None).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    std::fs::create_dir_all(session_dir.join("output")).unwrap();
    std::fs::write(
        session_dir.join("output/acp-events.jsonl"),
        "{\"seq\":1,\"ts\":\"2026-01-01T00:00:00Z\"}\n",
    )
    .unwrap();

    backdate_tree(&session_dir, 120);

    let reconciled =
        ensure_terminal_result_for_dead_active_session(project, &session_id, "session list")
            .unwrap();
    assert!(reconciled);

    let result = load_result(project, &session_id).unwrap().unwrap();
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
    assert!(result.summary.contains("session list"));
    assert!(
        result
            .artifacts
            .iter()
            .any(|artifact| artifact.path == "output/acp-events.jsonl"),
        "fallback should preserve output artifact references"
    );

    let persisted = load_session(project, &session_id).unwrap();
    assert_eq!(
        persisted.termination_reason.as_deref(),
        Some("orphaned_process")
    );
}

#[test]
fn ensure_terminal_result_for_dead_active_session_reconciles_fresh_output_without_live_process() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(project, Some("fresh-output"), None, None).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    std::fs::write(
        session_dir.join("output.log"),
        "recent output that should not block reconciliation\n",
    )
    .unwrap();

    let reconciled =
        ensure_terminal_result_for_dead_active_session(project, &session_id, "session list")
            .unwrap();
    assert!(
        reconciled,
        "fresh file writes without a live process should still synthesize a terminal result"
    );
    assert!(load_result(project, &session_id).unwrap().is_some());
}

#[cfg(unix)]
#[test]
fn handle_session_wait_waits_for_daemon_exit_before_returning_success() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::time::{Duration, Instant};

    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(project, Some("wait-daemon-exit"), None, Some("codex")).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    let result_path = session_dir.join(csa_session::result::RESULT_FILE_NAME);
    let stdout_path = session_dir.join("stdout.log");
    let script_path = td.path().join("delayed-session-writer.sh");
    let result_toml = toml::to_string_pretty(&make_result("success", 0)).unwrap();
    let directive = "<!-- CSA:NEXT_STEP cmd=\"echo done\" required=true -->";
    let script = format!(
        "#!/bin/sh\nset -eu\nsession_id=\"$1\"\nresult_path=\"$2\"\nstdout_path=\"$3\"\ncat > \"$result_path\" <<'EOF'\n{result_toml}\nEOF\nsleep 2\nprintf '%s\\n' '{directive}' > \"$stdout_path\"\n# Keep the session id in the argv/cmdline for liveness context matching.\nprintf '%s' \"$session_id\" >/dev/null\n"
    );
    std::fs::write(&script_path, script).unwrap();
    let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).unwrap();

    let mut child = Command::new(&script_path)
        .arg(&session_id)
        .arg(&result_path)
        .arg(&stdout_path)
        .spawn()
        .unwrap();
    std::fs::write(session_dir.join("daemon.pid"), child.id().to_string()).unwrap();

    let started = Instant::now();
    let exit_code = handle_session_wait(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        10,
    )
    .unwrap();
    let elapsed = started.elapsed();

    assert_eq!(exit_code, 0);
    assert!(
        elapsed >= Duration::from_secs(1),
        "wait should not return immediately after result.toml appears while daemon is still alive; elapsed={elapsed:?}"
    );
    assert!(
        std::fs::read_to_string(&stdout_path)
            .unwrap_or_default()
            .contains("CSA:NEXT_STEP"),
        "wait should return only after delayed stdout payload is present"
    );

    let status = child.wait().unwrap();
    assert!(status.success());
}

#[cfg(unix)]
#[test]
fn handle_session_wait_ignores_incomplete_result_while_daemon_alive() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session =
        create_session(project, Some("wait-partial-result"), None, Some("codex")).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    let result_path = session_dir.join(csa_session::result::RESULT_FILE_NAME);
    let stdout_path = session_dir.join("stdout.log");
    let script_path = td.path().join("partial-result-writer.sh");
    let result_toml = toml::to_string_pretty(&make_result("success", 0)).unwrap();
    let directive = "<!-- CSA:NEXT_STEP cmd=\"echo done\" required=true -->";
    let script = format!(
        "#!/bin/sh\nset -eu\nsession_id=\"$1\"\nresult_path=\"$2\"\nstdout_path=\"$3\"\n: > \"$result_path\"\nsleep 2\ncat > \"$result_path\" <<'EOF'\n{result_toml}\nEOF\nprintf '%s\\n' '{directive}' > \"$stdout_path\"\n# Keep the session id in the argv/cmdline for liveness context matching.\nprintf '%s' \"$session_id\" >/dev/null\n"
    );
    std::fs::write(&script_path, script).unwrap();
    let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).unwrap();

    let mut child = Command::new(&script_path)
        .arg(&session_id)
        .arg(&result_path)
        .arg(&stdout_path)
        .spawn()
        .unwrap();
    std::fs::write(session_dir.join("daemon.pid"), child.id().to_string()).unwrap();

    let exit_code = handle_session_wait(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
        10,
    )
    .unwrap();

    assert_eq!(exit_code, 0);
    assert!(
        std::fs::read_to_string(&stdout_path)
            .unwrap_or_default()
            .contains("CSA:NEXT_STEP"),
        "wait should continue until stdout content arrives after a partial result write"
    );

    let status = child.wait().unwrap();
    assert!(status.success());
}

#[cfg(unix)]
#[test]
fn handle_session_wait_prefers_daemon_completion_exit_code() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session =
        create_session(project, Some("wait-completion-packet"), None, Some("codex")).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    save_result(project, &session_id, &make_result("success", 0)).unwrap();
    std::fs::write(
        session_dir.join("daemon-completion.toml"),
        "exit_code = 1\nstatus = \"failure\"\n",
    )
    .unwrap();

    let exit_code =
        handle_session_wait(session_id, Some(project.to_string_lossy().into_owned()), 1).unwrap();

    assert_eq!(
        exit_code, 1,
        "session wait should return the daemon's final exit code, not the intermediate result.toml exit code"
    );
}

#[cfg(unix)]
#[test]
fn handle_session_wait_returns_pre_exec_failure_without_timeout_packet() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session =
        create_session(project, Some("wait-pre-exec-failure"), None, Some("codex")).unwrap();
    let session_id = session.meta_session_id;

    let failure = SessionResult {
        summary: "pre-exec: --extra-writable validation failed: rejected paths [\"/ssd\"]"
            .to_string(),
        ..make_result("failure", 1)
    };
    save_result(project, &session_id, &failure).unwrap();

    let exit_code =
        handle_session_wait(session_id, Some(project.to_string_lossy().into_owned()), 1).unwrap();

    assert_eq!(exit_code, 1);
}

#[cfg(unix)]
#[test]
fn persist_daemon_completion_from_env_writes_packet_for_seeded_session() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();

    let session = create_session(project, Some("completion-packet"), None, Some("codex")).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();

    seed_daemon_session_env(&session_id, Some(project.to_string_lossy().as_ref()));
    persist_daemon_completion_from_env(17);

    let packet = std::fs::read_to_string(session_dir.join("daemon-completion.toml")).unwrap();
    assert!(packet.contains("exit_code = 17"));
    assert!(packet.contains("status = \"failure\""));
}

#[cfg(unix)]
#[test]
fn synthesized_wait_next_step_returns_directive_for_clean_cumulative_review() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();
    let session = create_session(project, Some("wait-next-step"), None, Some("codex")).unwrap();
    let session_dir = get_session_dir(project, &session.meta_session_id).unwrap();

    std::fs::write(
        session_dir.join("review_meta.json"),
        r#"{
  "session_id": "01TEST",
  "head_sha": "deadbeef",
  "decision": "pass",
  "verdict": "CLEAN",
  "tool": "codex",
  "scope": "range:main...HEAD",
  "exit_code": 0,
  "fix_attempted": false,
  "fix_rounds": 0,
  "timestamp": "2026-04-01T00:00:00Z"
}"#,
    )
    .unwrap();

    let directive = synthesized_wait_next_step(&session_dir)
        .unwrap()
        .expect("directive should be synthesized");
    assert!(directive.contains("CSA:NEXT_STEP"));
    assert!(directive.contains("pr-bot"));
}

#[cfg(unix)]
#[test]
fn synthesized_wait_next_step_skips_non_cumulative_or_existing_directive() {
    let td = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.lock().expect("session env lock poisoned");
    let state_home = td.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = td.path();
    let session =
        create_session(project, Some("wait-next-step-skip"), None, Some("codex")).unwrap();
    let session_dir = get_session_dir(project, &session.meta_session_id).unwrap();

    std::fs::write(
        session_dir.join("review_meta.json"),
        r#"{
  "session_id": "01TEST",
  "head_sha": "deadbeef",
  "decision": "pass",
  "verdict": "CLEAN",
  "tool": "codex",
  "scope": "files:crates/csa-hooks/src/",
  "exit_code": 0,
  "fix_attempted": false,
  "fix_rounds": 0,
  "timestamp": "2026-04-01T00:00:00Z"
}"#,
    )
    .unwrap();
    assert!(synthesized_wait_next_step(&session_dir).unwrap().is_none());

    std::fs::write(
        session_dir.join("review_meta.json"),
        r#"{
  "session_id": "01TEST",
  "head_sha": "deadbeef",
  "decision": "pass",
  "verdict": "CLEAN",
  "tool": "codex",
  "scope": "base:main",
  "exit_code": 0,
  "fix_attempted": false,
  "fix_rounds": 0,
  "timestamp": "2026-04-01T00:00:00Z"
}"#,
    )
    .unwrap();
    std::fs::write(
        session_dir.join("stdout.log"),
        "<!-- CSA:NEXT_STEP cmd=\"custom\" required=false -->\n",
    )
    .unwrap();
    assert!(synthesized_wait_next_step(&session_dir).unwrap().is_none());
}
