use super::*;

#[cfg(unix)]
#[test]
fn ensure_terminal_result_for_dead_active_session_is_noop_when_result_exists() {
    let td = tempdir().unwrap();
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
    let project = td.path();

    let created = create_session(project, Some("json-reconcile"), None, None).unwrap();
    let session_id = created.meta_session_id.clone();
    let session_dir = get_session_dir(project, &session_id).unwrap();
    backdate_tree(&session_dir, 120);

    let session = load_session(project, &session_id).unwrap();
    let value = session_to_json(project, &session);
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
    let project = td.path();

    let created = create_session(project, Some("legacy-session"), None, None).unwrap();
    let session_id = created.meta_session_id;
    let primary_root = get_session_root(project).unwrap();
    let primary_session_dir = primary_root.join("sessions").join(&session_id);
    let legacy_sessions_dir = super::legacy_sessions_dir_from_primary_root(&primary_root)
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
