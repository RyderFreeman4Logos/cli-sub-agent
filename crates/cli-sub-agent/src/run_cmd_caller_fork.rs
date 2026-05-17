//! `--fork-from-caller` resolution for CSA-lite Phase 1 (issue #1432).
//!
//! Detects the caller's Claude session via [`csa_session::detect_caller_session`],
//! extracts a token-budgeted conversation prefix via [`csa_acp::PrefixExtractor`]
//! (when the `acp` feature is enabled), and returns a [`ForkResolution`] whose
//! `context_prefix` carries the extracted text. The result flows into
//! `execute_run_loop` as the initial fork resolution so the existing
//! prepend-to-prompt path injects the caller's history.
//!
//! Graceful degradation: if detection or extraction fails, this returns
//! `None` and emits a `tracing::warn!`. The caller-side `handle_run`
//! continues with a normal cold start.

use csa_config::ProjectConfig;
#[cfg(feature = "acp")]
use tracing::info;
use tracing::warn;

use crate::run_cmd_fork::ForkResolution;

/// Resolve `--fork-from-caller` into an optional [`ForkResolution`].
///
/// Returns `None` when no caller session can be detected, when prefix
/// extraction is unavailable (no `acp` feature), or when extraction fails.
pub(crate) fn resolve_fork_from_caller(config: Option<&ProjectConfig>) -> Option<ForkResolution> {
    let caller = csa_session::detect_caller_session()?;
    let budget = config
        .map(|c| c.session.resolved_fork_prefix_budget())
        .unwrap_or(csa_config::DEFAULT_FORK_PREFIX_BUDGET_TOKENS);

    extract_caller_prefix(&caller, budget)
}

#[cfg(feature = "acp")]
fn extract_caller_prefix(
    caller: &csa_session::CallerSessionInfo,
    budget: u32,
) -> Option<ForkResolution> {
    let config = csa_acp::PrefixConfig {
        budget_tokens: budget as usize,
        skip_tool_results: true,
    };
    let extractor = csa_acp::PrefixExtractor::new(config);
    match extractor.extract_prefix(&caller.jsonl_path) {
        Ok(prefix) => {
            info!(
                caller_session = %caller.session_id,
                tokens = prefix.token_count,
                messages = prefix.message_count,
                truncated = prefix.truncated,
                budget,
                "caller fork: extracted conversation prefix"
            );
            Some(ForkResolution {
                provider_session_id: None,
                context_prefix: Some(prefix.content),
                source_session_id: caller.session_id.clone(),
                source_provider_session_id: Some(caller.session_id.clone()),
            })
        }
        Err(err) => {
            warn!(
                caller_session = %caller.session_id,
                jsonl = %caller.jsonl_path.display(),
                error = %err,
                "caller fork: prefix extraction failed; falling back to cold start"
            );
            None
        }
    }
}

#[cfg(not(feature = "acp"))]
fn extract_caller_prefix(
    caller: &csa_session::CallerSessionInfo,
    _budget: u32,
) -> Option<ForkResolution> {
    warn!(
        caller_session = %caller.session_id,
        "caller fork: prefix extraction requires the `acp` feature; falling back to cold start"
    );
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
    use csa_session::CallerSessionInfo;
    use std::path::PathBuf;

    fn fake_caller(jsonl_path: PathBuf) -> CallerSessionInfo {
        CallerSessionInfo {
            session_id: "11111111-2222-3333-4444-555555555555".to_string(),
            session_dir: jsonl_path.parent().unwrap().to_path_buf(),
            jsonl_path,
            provider: "claude".to_string(),
        }
    }

    #[cfg(feature = "acp")]
    #[test]
    fn extract_caller_prefix_returns_resolution_for_valid_jsonl() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let jsonl = tmp.path().join("session.jsonl");
        std::fs::write(
            &jsonl,
            r#"{"type":"user","message":{"role":"user","content":"hello caller"}}
"#,
        )
        .expect("write fixture");

        let caller = fake_caller(jsonl);
        let resolution = extract_caller_prefix(&caller, 32_768)
            .expect("extraction should succeed for valid JSONL");
        assert_eq!(resolution.source_session_id, caller.session_id);
        assert_eq!(
            resolution.source_provider_session_id.as_deref(),
            Some(caller.session_id.as_str())
        );
        assert!(resolution.provider_session_id.is_none());
        let prefix = resolution.context_prefix.expect("context_prefix populated");
        assert!(prefix.contains("hello caller"));
    }

    #[cfg(feature = "acp")]
    #[test]
    fn extract_caller_prefix_returns_none_for_missing_jsonl() {
        let caller = fake_caller(PathBuf::from("/nonexistent/csa-test-caller-fork.jsonl"));
        assert!(extract_caller_prefix(&caller, 32_768).is_none());
    }

    #[cfg(not(feature = "acp"))]
    #[test]
    fn extract_caller_prefix_returns_none_without_acp_feature() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let jsonl = tmp.path().join("session.jsonl");
        std::fs::write(&jsonl, "{}\n").expect("write fixture");
        let caller = fake_caller(jsonl);
        assert!(extract_caller_prefix(&caller, 32_768).is_none());
    }

    /// Integration: when `CLAUDE_SESSION_ID` points at a non-existent
    /// session, `resolve_fork_from_caller` must degrade gracefully
    /// (return None) rather than propagating an error.
    #[test]
    fn resolve_fork_from_caller_returns_none_when_session_missing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("projects")).expect("mkdir projects");
        let claude_root = tmp.path().to_string_lossy().to_string();

        // Hold the process-wide env lock for both var mutations.
        let _lock = TEST_ENV_LOCK.clone().blocking_lock_owned();
        let _claude_root_guard = ScopedEnvVarRestore::set("CLAUDE_CONFIG_DIR", claude_root);
        let _session_guard =
            ScopedEnvVarRestore::set("CLAUDE_SESSION_ID", "deadbeef-0000-0000-0000-000000000000");

        assert!(resolve_fork_from_caller(None).is_none());
    }
}
