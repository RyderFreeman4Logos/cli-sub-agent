use anyhow::Result;

use crate::session_cmds::resolve_session_prefix_with_fallback;

/// Handle `csa session tool-output <session> [index] [--list]`.
pub(crate) fn handle_session_tool_output(
    session: String,
    index: Option<u32>,
    list: bool,
    cd: Option<String>,
) -> Result<()> {
    use csa_session::tool_output_store::ToolOutputStore;

    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_fallback(&project_root, &session)?;
    let session_id = resolved.session_id;
    let session_dir = csa_session::get_session_dir(&project_root, &session_id)?;

    let store = ToolOutputStore::open_readonly(&session_dir);

    if list || index.is_none() {
        let manifest = store.read_manifest()?;
        if manifest.entries.is_empty() {
            println!("No compressed tool outputs for session {session_id}.");
            return Ok(());
        }
        println!("Compressed tool outputs for session {session_id}:");
        for entry in &manifest.entries {
            println!(
                "  [{:>3}] {} bytes -> {}",
                entry.index, entry.original_bytes, entry.path
            );
        }
        return Ok(());
    }

    let idx = index.expect("index required when not listing");
    let content = store.load(idx)?;
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    std::io::Write::write_all(&mut handle, &content)?;
    Ok(())
}
