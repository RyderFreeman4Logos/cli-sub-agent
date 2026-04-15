use std::collections::HashMap;

#[test]
fn apply_review_target_dir_routes_review_sessions_off_repo() {
    let session_dir = std::path::Path::new("/tmp/csa-state/sessions/01TEST");
    let mut env = HashMap::new();
    env.insert(
        "CARGO_TARGET_DIR".to_string(),
        "/repo/legacy-review-target".to_string(),
    );

    crate::pipeline_env::apply_review_target_dir(Some("review"), session_dir, &mut env);

    assert_eq!(
        env.get("CARGO_TARGET_DIR").map(String::as_str),
        Some("/tmp/csa-state/sessions/01TEST/target")
    );
}

#[test]
fn apply_review_target_dir_leaves_non_review_sessions_unchanged() {
    let session_dir = std::path::Path::new("/tmp/csa-state/sessions/01TEST");
    let mut env = HashMap::new();
    env.insert(
        "CARGO_TARGET_DIR".to_string(),
        "/repo/legacy-review-target".to_string(),
    );

    crate::pipeline_env::apply_review_target_dir(Some("run"), session_dir, &mut env);

    assert_eq!(
        env.get("CARGO_TARGET_DIR").map(String::as_str),
        Some("/repo/legacy-review-target")
    );
}
