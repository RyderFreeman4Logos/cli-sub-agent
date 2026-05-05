use super::*;
use std::collections::HashMap;

#[test]
fn review_readonly_prompt_detection_skips_fix_prompts() {
    assert!(review_prompt_is_readonly(
        "Use the csa-review skill. scope=uncommitted, mode=review-only"
    ));
    assert!(!review_prompt_is_readonly(
        "Fix round 1/3. Fix all issues found in the review."
    ));
}

#[test]
fn with_readonly_session_env_injects_flag() {
    let mut base = HashMap::new();
    base.insert("EXISTING".to_string(), "value".to_string());

    let env = with_readonly_session_env(Some(&base), true).expect("env map");

    assert_eq!(env.get("EXISTING").map(String::as_str), Some("value"));
    assert_eq!(
        env.get(CSA_READONLY_SESSION_ENV).map(String::as_str),
        Some("1")
    );
}
