use super::*;
use crate::session_cmds::resolve_session_prefix_with_global_fallback;

#[test]
fn resolve_session_prefix_with_global_fallback_accepts_metadata_only_exact_session_dir() {
    let td = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&td);
    let caller_project = td.path().join("caller");
    let owner_project = td.path().join("owner");
    std::fs::create_dir_all(&caller_project).unwrap();
    std::fs::create_dir_all(&owner_project).unwrap();

    let session = create_session(&owner_project, Some("metadata-only"), None, Some("codex"))
        .expect("create owner session");
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(&owner_project, &session_id).expect("session dir");
    std::fs::remove_file(session_dir.join("state.toml")).expect("remove registry state");

    let resolved =
        resolve_session_prefix_with_global_fallback(&caller_project, &session_id).unwrap();
    assert_eq!(resolved.session_id, session_id);
    assert_eq!(
        resolved.sessions_dir,
        session_dir.parent().expect("sessions dir")
    );
    assert_eq!(
        resolved.foreign_project_root,
        Some(std::fs::canonicalize(&owner_project).unwrap())
    );
}

#[test]
fn resolve_session_prefix_with_global_fallback_does_not_guess_unknown_exact_id() {
    let td = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&td);
    let project = td.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let unknown_id = "01ARZ3NDEKTSV4RRFFQ69G5FBG";

    let err = resolve_session_prefix_with_global_fallback(&project, unknown_id)
        .expect_err("unknown exact id must not be guessed");
    let message = err.to_string();
    assert!(message.contains("no session registration was found"));
    assert!(message.contains("CSA:SESSION_STARTED"));
}
