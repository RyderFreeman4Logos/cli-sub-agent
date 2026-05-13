use super::*;

#[test]
fn initial_response_timeout_promotes_to_wall_timeout() {
    assert_eq!(
        resolve_effective_initial_response_timeout_for_tool(None, None, None, Some(1800), "codex"),
        Some(1800)
    );
}

#[test]
fn initial_response_timeout_respects_explicit_initial_response_timeout() {
    assert_eq!(
        resolve_effective_initial_response_timeout_for_tool(
            None,
            Some(90),
            None,
            Some(1800),
            "codex",
        ),
        Some(90)
    );
}

#[test]
fn initial_response_timeout_keeps_disabled_timeout_disabled() {
    assert_eq!(
        resolve_effective_initial_response_timeout_for_tool(
            None,
            None,
            Some(1800),
            Some(1800),
            "codex",
        ),
        None
    );
}
