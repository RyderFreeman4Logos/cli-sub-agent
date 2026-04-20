use anyhow::Result;
use csa_executor::Executor;
use csa_session::{MetaSessionState, ToolState, save_session};

pub(super) async fn ensure_tool_state_initialized(
    session: &mut MetaSessionState,
    executor: &Executor,
    resolved_provider_session_id: &Option<String>,
) -> Result<Option<ToolState>> {
    let tool_name = executor.tool_name().to_string();
    let mut dirty = false;

    if !session.tools.contains_key(&tool_name) {
        session.tools.insert(
            tool_name.clone(),
            ToolState {
                provider_session_id: resolved_provider_session_id.clone(),
                last_action_summary: String::new(),
                last_exit_code: 0,
                updated_at: chrono::Utc::now(),
                tool_version: None,
                token_usage: None,
            },
        );
        dirty = true;
    }

    if let Some(tool_state) = session.tools.get_mut(&tool_name) {
        if tool_state.provider_session_id.is_none() && resolved_provider_session_id.is_some() {
            tool_state.provider_session_id = resolved_provider_session_id.clone();
            tool_state.updated_at = chrono::Utc::now();
            dirty = true;
        }

        if tool_state.tool_version.is_none() {
            let detected = crate::tool_version::detect_tool_version(executor).await;
            if detected.is_some() {
                tool_state.tool_version = detected;
                tool_state.updated_at = chrono::Utc::now();
                dirty = true;
            }
        }
    }

    if dirty {
        save_session(session)?;
    }

    Ok(session.tools.get(&tool_name).cloned())
}
