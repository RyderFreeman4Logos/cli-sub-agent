use super::*;

fn result(exit_code: i32, terminal_reason: Option<&str>) -> csa_process::ExecutionResult {
    csa_process::ExecutionResult {
        exit_code,
        terminal_reason: terminal_reason.map(str::to_string),
        ..Default::default()
    }
}

#[test]
fn initial_session_status_preserves_synthetic_interruptions() {
    assert_eq!(
        initial_session_status(&result(143, Some("sigterm"))),
        "interrupted"
    );
    assert_eq!(
        initial_session_status(&result(130, Some("sigint"))),
        "interrupted"
    );
    assert_eq!(
        initial_session_status(&result(124, Some("timeout"))),
        "timed_out"
    );
    assert_eq!(initial_session_status(&result(137, None)), "signal");
}
