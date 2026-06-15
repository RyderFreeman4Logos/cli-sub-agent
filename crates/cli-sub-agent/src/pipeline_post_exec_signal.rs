use csa_session::{MetaSessionState, SessionResult};

pub(crate) fn record_signal_session_metadata(
    session: &mut MetaSessionState,
    tool_name: &str,
    result: &SessionResult,
    terminal_reason: Option<&str>,
) {
    if !matches!(result.exit_code, 137 | 143) {
        return;
    }
    session.termination_reason = Some(
        result
            .kill_hint
            .as_deref()
            .or(terminal_reason)
            .unwrap_or("signal")
            .to_string(),
    );
    if let Some(tool_state) = session.tools.get_mut(tool_name) {
        tool_state.last_exit_code = result.exit_code;
        tool_state.last_action_summary = result.summary.clone();
    }
}
