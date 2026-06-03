use std::fs;

use anyhow::Result;

use crate::session_cmds::{
    ensure_terminal_result_for_dead_active_session, format_file_size,
    resolve_session_prefix_with_fallback,
};

pub(crate) fn handle_session_artifacts(session: String, cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_fallback(&project_root, &session)?;
    let resolved_id = resolved.session_id;
    if let Err(err) = ensure_terminal_result_for_dead_active_session(
        &project_root,
        &resolved_id,
        "session artifacts",
    ) {
        tracing::warn!(
            session_id = %resolved_id,
            error = %err,
            "Failed to reconcile dead Active session in session artifacts"
        );
    }
    let session_dir = csa_session::get_session_dir(&project_root, &resolved_id)?;
    let _ = crate::session_observability::refresh_and_repair_result(&project_root, &resolved_id);
    let output_dir = session_dir.join("output");

    if let Some(index) = csa_session::load_output_index(&session_dir)? {
        println!(
            "Structured output ({} sections, ~{} tokens):",
            index.sections.len(),
            index.total_tokens
        );
        for section in &index.sections {
            let size_str = if let Some(ref fp) = section.file_path {
                let path = output_dir.join(fp);
                match fs::metadata(&path) {
                    Ok(meta) => format_file_size(meta.len()),
                    Err(_) => "missing".to_string(),
                }
            } else {
                "-".to_string()
            };
            println!(
                "  {:<20}  {:<30}  ~{}tok  {}",
                section.id, section.title, section.token_estimate, size_str
            );
        }
        println!();
    }

    if output_dir.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(&output_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .collect();
        entries.sort_by_key(|e| e.file_name());

        if entries.is_empty() {
            eprintln!("No artifacts for session '{resolved_id}'");
        } else {
            println!("Files:");
            for entry in &entries {
                let path = entry.path();
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                println!("  {:<40}  {}", name, format_file_size(size));
            }
        }
    } else {
        eprintln!("No artifacts for session '{resolved_id}'");
    }

    Ok(())
}
