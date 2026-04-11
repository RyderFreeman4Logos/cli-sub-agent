use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

pub(crate) const ACP_PAYLOAD_DEBUG_ENV: &str = "CSA_DEBUG_ACP_PAYLOAD";
const ACP_PAYLOAD_DEBUG_REL_PATH: &str = "output/acp-payload-debug.json";

pub(super) struct AcpPayloadDebugRequest<'a> {
    pub(super) env: &'a HashMap<String, String>,
    pub(super) tool_name: &'a str,
    pub(super) command: &'a str,
    pub(super) args: &'a [String],
    pub(super) working_dir: &'a Path,
    pub(super) resume_session_id: Option<&'a str>,
    pub(super) system_prompt: Option<&'a str>,
    pub(super) session_meta: Option<&'a serde_json::Map<String, serde_json::Value>>,
    pub(super) prompt: &'a str,
}

#[derive(Debug, Serialize)]
struct AcpPayloadDebug<'a> {
    tool_name: &'a str,
    command: &'a str,
    args: &'a [String],
    working_dir: String,
    resume_session_id: Option<&'a str>,
    system_prompt: Option<&'a str>,
    session_meta: Option<&'a serde_json::Map<String, serde_json::Value>>,
    prompt_chars: usize,
    prompt_preview: String,
    prompt: &'a str,
}

fn acp_payload_debug_enabled() -> bool {
    std::env::var(ACP_PAYLOAD_DEBUG_ENV)
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(false)
}

pub(super) fn maybe_write_acp_payload_debug(
    request: AcpPayloadDebugRequest<'_>,
) -> Option<PathBuf> {
    if !acp_payload_debug_enabled() {
        return None;
    }

    let session_dir = request.env.get("CSA_SESSION_DIR")?;
    let debug_path = Path::new(session_dir).join(ACP_PAYLOAD_DEBUG_REL_PATH);
    if let Some(parent) = debug_path.parent()
        && fs::create_dir_all(parent).is_err()
    {
        return None;
    }

    let payload = AcpPayloadDebug {
        tool_name: request.tool_name,
        command: request.command,
        args: request.args,
        working_dir: request.working_dir.display().to_string(),
        resume_session_id: request.resume_session_id,
        system_prompt: request.system_prompt,
        session_meta: request.session_meta,
        prompt_chars: request.prompt.chars().count(),
        prompt_preview: request.prompt.chars().take(2000).collect(),
        prompt: request.prompt,
    };
    let serialized = serde_json::to_string_pretty(&payload).ok()?;
    fs::write(&debug_path, format!("{serialized}\n")).ok()?;
    Some(debug_path)
}
