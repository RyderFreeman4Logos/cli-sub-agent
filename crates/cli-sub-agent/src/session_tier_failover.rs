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
    project_root: &Path,
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
            "{}",
            render_pending_tier_failover_handoff(session_id, project_root)
        );
    }
}

fn render_pending_tier_failover_handoff(session_id: &str, project_root: &Path) -> String {
    let wait_command =
        crate::daemon_caller_hints::resolve_session_wait_command(session_id, project_root, None);
    debug_assert!(
        wait_command.command().is_none(),
        "tier fallback has no trusted provider and must not emit a runnable wait command"
    );
    format!(
        "Session {session_id} is awaiting tier fallback; select a configured provider before retrying.\n{}",
        wait_command.provider_selection_hint()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK, isolate_user_config};

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

    #[test]
    fn pending_tier_failover_handoff_requires_selecting_configured_wait_provider() {
        let _lock = TEST_ENV_LOCK.clone().blocking_lock_owned();
        let temp = tempfile::tempdir().expect("tempdir");
        let (_home_guard, _config_guard) = isolate_user_config(temp.path());
        let _ambient_provider =
            ScopedEnvVarRestore::set("HERMES_MODEL_PROVIDER", "ambient-conflict");
        let config_path =
            csa_config::ProjectConfig::user_config_path().expect("resolve user config path");
        std::fs::create_dir_all(config_path.parent().expect("config parent"))
            .expect("create config parent");
        std::fs::write(
            config_path,
            "[kv_cache.provider_ttls]\nopenai = 17\nxai = 3300\ndisabled = 0\n",
        )
        .expect("write provider config");

        let text = render_pending_tier_failover_handoff("01KAS6M5XG7V4M4M6YDRS7P8R9", temp.path());

        assert!(
            text.contains("CSA:CALLER_HINT action=\"select_wait_provider\""),
            "tier fallback without trusted provider must require explicit provider selection: {text}"
        );
        assert!(
            text.contains("legal_keys=\"openai,xai\""),
            "only positive configured provider keys must be actionable: {text}"
        );
        assert!(
            !text.contains("csa session wait --session 01KAS6M5XG7V4M4M6YDRS7P8R9"),
            "tier fallback must never emit a bare providerless wait command: {text}"
        );
        assert!(
            !text.contains("ambient-conflict"),
            "tier fallback must not infer a provider from ambient environment: {text}"
        );
    }
}
