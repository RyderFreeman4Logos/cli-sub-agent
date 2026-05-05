use super::*;
use std::collections::HashMap;

#[test]
fn debate_readonly_env_injects_flag() {
    let mut base = HashMap::new();
    base.insert("EXISTING".to_string(), "value".to_string());

    let env = with_readonly_session_env(Some(&base), true).expect("env map");

    assert_eq!(env.get("EXISTING").map(String::as_str), Some("value"));
    assert_eq!(
        env.get("CSA_READONLY_SESSION").map(String::as_str),
        Some("1")
    );
}
