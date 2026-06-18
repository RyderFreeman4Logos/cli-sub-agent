use super::*;
use std::cell::Cell;

#[test]
fn csa_timeout_diagnostic_reports_requested_and_effective_timeout() {
    let called = Cell::new(false);
    let timeout = TimeoutDiagnostics {
        requested_timeout_seconds: Some(10_800),
        idle_timeout_seconds: Some(10_800),
        initial_response_timeout_seconds: Some(45),
    };
    let diagnostic = diagnose_signal_kill_with_events(
        137,
        Some("initial_response_timeout"),
        None,
        None,
        Some(timeout),
        || {
            called.set(true);
            KillSignalObservations::default()
        },
    )
    .expect("timeout signal should produce diagnostic");

    assert_eq!(diagnostic.hint, KillHint::CsaTimeout);
    assert!(!called.get(), "timeout metadata should skip memory checks");
    let line = diagnostic
        .stderr_line()
        .expect("timeout signal should render timeout details");
    assert!(line.contains("termination_reason=initial_response_timeout"));
    assert!(line.contains("requested_timeout_seconds=10800"));
    assert!(line.contains("effective_timeout_kind=initial_response_timeout"));
    assert!(line.contains("effective_timeout_seconds=45"));
    assert!(line.contains("effective_timeout_source=initial_response_timeout"));
    assert!(line.contains("idle_timeout_seconds=10800"));
}
