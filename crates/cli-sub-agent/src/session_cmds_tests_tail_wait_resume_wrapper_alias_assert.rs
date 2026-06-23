pub(super) fn assert_fix_finding_wrapper_summary(
    summary: &str,
    wrapper_id: &str,
    fix_session_id: &str,
    original_review_id: &str,
) {
    for expected in [
        format!("Session: {wrapper_id}"),
        format!("Target session: {fix_session_id}"),
        format!(
            "Alias: kind=resume-wrapper requested_session_id={wrapper_id} target_session_id={fix_session_id}"
        ),
    ] {
        assert!(summary.contains(&expected), "{summary}");
    }
    for unexpected in [
        format!("Session: {fix_session_id}"),
        format!("Session: {original_review_id}"),
    ] {
        assert!(!summary.contains(&unexpected), "{summary}");
    }
}
