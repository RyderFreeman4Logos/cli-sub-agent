use anyhow::Result;
use csa_session::load_session;

use super::resolve_session_prefix_with_fallback;

pub(crate) fn handle_session_compress(session: String, cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_fallback(&project_root, &session)?;
    let resolved_id = resolved.session_id;
    let session_state = load_session(&project_root, &resolved_id)?;

    // Find the most recently used tool in this session
    let (tool_name, _tool_state) = session_state
        .tools
        .iter()
        .max_by_key(|(_, state)| &state.updated_at)
        .ok_or_else(|| anyhow::anyhow!("Session '{resolved_id}' has no tool history"))?;

    let compress_cmd = match tool_name.as_str() {
        "gemini-cli" | "antigravity-cli" => "/compress",
        _ => "/compact",
    };

    println!("Session {resolved_id} uses tool: {tool_name}");
    println!("Compress command: {compress_cmd}");
    println!();
    println!("To compress, resume the session and send the command:");
    println!(
        "  csa run --sa-mode <true|false> --tool {tool_name} --session {resolved_id} \"{compress_cmd}\""
    );
    println!();
    println!("Note: context status will be updated after the tool confirms compression.");

    // Do NOT mark is_compacted = true here. The actual compression must be
    // performed by the tool. Status should only be updated after `csa run`
    // executes the compress command and succeeds.

    Ok(())
}
