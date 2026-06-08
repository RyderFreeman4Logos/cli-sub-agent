use std::path::Path;

pub(crate) const TIER_FAILOVER_SUPERSEDED_STATUS: &str = "tier_failover_superseded";

pub(crate) fn is_tier_failover_superseded_status(status: &str) -> bool {
    status
        .trim()
        .eq_ignore_ascii_case(TIER_FAILOVER_SUPERSEDED_STATUS)
}

pub(crate) fn is_tier_failover_superseded_result(result: &csa_session::SessionResult) -> bool {
    is_tier_failover_superseded_status(&result.status)
}

pub(crate) fn is_pending_tier_failover_handoff(
    session_dir: &Path,
    result: &csa_session::SessionResult,
) -> bool {
    is_tier_failover_superseded_result(result)
        && (csa_process::ToolLiveness::has_live_process(session_dir)
            || csa_process::ToolLiveness::daemon_pid_is_alive(session_dir)
            || csa_process::ToolLiveness::is_alive(session_dir))
}

pub(crate) fn emit_pending_tier_failover_handoff(
    session_id: &str,
    result: &csa_session::SessionResult,
    json: bool,
) {
    if json {
        println!(
            "{}",
            serde_json::json!({
                "session_id": session_id,
                "status": "tier_failover_handoff_pending",
                "superseded_status": result.status,
                "exit_code": result.exit_code,
                "tool": result.tool,
            })
        );
    } else {
        eprintln!(
            "Session {session_id} is awaiting tier fallback; retry `csa session wait --session {session_id}` for the fallback result."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_result_hides_tier_failover_superseded_result_while_liveness_present() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = csa_session::SessionResult {
            status: TIER_FAILOVER_SUPERSEDED_STATUS.to_string(),
            exit_code: 1,
            summary: "status: 400".to_string(),
            tool: "gemini-cli".to_string(),
            ..Default::default()
        };
        std::fs::write(
            tmp.path().join("stderr.log"),
            "fallback codex review is still running\n",
        )
        .expect("write liveness");

        assert!(
            is_pending_tier_failover_handoff(tmp.path(), &result),
            "live superseded tier attempts are handoff-pending, not terminal result failures"
        );
    }
}
