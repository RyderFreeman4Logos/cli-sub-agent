//! Fork resolution and return-packet handling for `csa run`.
//!
//! Extracted from `run_cmd.rs` to keep module sizes manageable.

use std::path::Path;

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use csa_executor::transport::{ForkMethod, ForkRequest, TransportFactory};
use csa_session::{
    RETURN_PACKET_SECTION_ID, ReturnPacketRef, load_output_index, parse_return_packet, read_section,
    validate_return_packet_path,
};

/// Result of resolving a fork request before execution.
pub(crate) struct ForkResolution {
    /// The forked provider session ID (Native fork only).
    pub(crate) provider_session_id: Option<String>,
    /// Context summary to prepend to prompt (Soft fork only).
    pub(crate) context_prefix: Option<String>,
    /// The CSA session ID that was forked from.
    pub(crate) source_session_id: String,
    /// The provider session ID of the source (used to set fork_provider_session_id in genealogy).
    pub(crate) source_provider_session_id: Option<String>,
}

/// Resolve a fork from a source session: run the transport-level fork and return
/// the information needed to create a new CSA session with fork genealogy.
///
/// For soft forks (cross-tool or non-claude-code targets), tool-lock is NOT enforced
/// because soft forks only copy context from the parent session and do not require
/// tool ownership. Native forks (same tool) still enforce tool-lock via
/// `resolve_resume_session`.
pub(crate) async fn resolve_fork(
    source_session_id: &str,
    tool_name: &str,
    project_root: &Path,
    codex_auto_trust: bool,
) -> Result<ForkResolution> {
    // Determine if source session uses a different tool than the target.
    // Cross-tool forks must always use soft fork (context summary injection)
    // because native fork requires the same tool's provider session.
    // When metadata is missing (older/migrated sessions), default to cross-tool
    // (soft fork) as the safe fallback — native fork would fail without metadata.
    let source_tool = csa_session::load_metadata(project_root, source_session_id)
        .ok()
        .flatten()
        .map(|m| m.tool);
    let is_cross_tool = source_tool.as_deref() != Some(tool_name);

    let fork_method = if is_cross_tool {
        ForkMethod::Soft
    } else {
        TransportFactory::fork_method_for_tool(tool_name)
    };

    let resolution = match fork_method {
        ForkMethod::Native => {
            // Native fork requires the same tool's provider session — enforce tool-lock.
            csa_session::resolve_resume_session(project_root, source_session_id, tool_name)?
        }
        ForkMethod::Soft => {
            // Soft fork only reads context files — skip tool-lock enforcement.
            csa_session::resolve_fork_source(project_root, source_session_id)?
        }
    };
    let source_csa_id = resolution.meta_session_id.clone();
    let source_provider_id = resolution.provider_session_id.clone();

    let session_dir = csa_session::get_session_dir(project_root, &source_csa_id)?;

    let fork_request = ForkRequest {
        tool_name: tool_name.to_string(),
        fork_method: Some(fork_method),
        codex_auto_trust,
        provider_session_id: source_provider_id.clone(),
        parent_csa_session_id: source_csa_id.clone(),
        parent_session_dir: session_dir.clone(),
        working_dir: project_root.to_path_buf(),
        timeout: std::time::Duration::from_secs(60),
    };

    let fork_info = TransportFactory::fork_session(&fork_request).await;

    if !fork_info.success {
        let notes = fork_info.notes.unwrap_or_default();
        anyhow::bail!(
            "Fork failed for session {} ({:?}): {}",
            source_csa_id,
            fork_info.method,
            notes
        );
    }

    info!(
        source = %source_csa_id,
        method = ?fork_info.method,
        new_provider_session = ?fork_info.new_session_id,
        notes = ?fork_info.notes,
        "Session fork completed"
    );

    // For soft fork, we need to read the context summary to prepend to the prompt
    let context_prefix = if matches!(fork_info.method, ForkMethod::Soft) {
        match csa_session::soft_fork_session(&session_dir, &source_csa_id) {
            Ok(ctx) => Some(ctx.context_summary),
            Err(e) => {
                warn!("Soft fork context extraction failed (non-fatal): {e}");
                None
            }
        }
    } else {
        None
    };

    Ok(ForkResolution {
        provider_session_id: fork_info.new_session_id,
        context_prefix,
        source_session_id: source_csa_id,
        source_provider_session_id: source_provider_id,
    })
}

/// Load the return packet and its reference from a child session's structured output.
pub(crate) fn load_child_return_packet(
    project_root: &Path,
    child_session_id: &str,
) -> Result<(csa_session::ReturnPacket, ReturnPacketRef)> {
    let child_session_dir = csa_session::get_session_dir(project_root, child_session_id)?;
    let section_content = read_section(&child_session_dir, RETURN_PACKET_SECTION_ID)?
        .ok_or_else(|| anyhow::anyhow!("child session missing return-packet section"))?;

    let packet = parse_return_packet(&section_content)?;
    for changed in &packet.changed_files {
        if !validate_return_packet_path(&changed.path, project_root) {
            anyhow::bail!(
                "return packet changed file path escapes project root: {}",
                changed.path
            );
        }
    }

    let index = load_output_index(&child_session_dir)?
        .ok_or_else(|| anyhow::anyhow!("missing output/index.toml for child session"))?;
    let section = index
        .sections
        .iter()
        .find(|s| s.id == RETURN_PACKET_SECTION_ID)
        .ok_or_else(|| anyhow::anyhow!("return-packet section not indexed"))?;
    let file_path = section
        .file_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("return-packet section has no file_path"))?;

    let output_dir = child_session_dir
        .join("output")
        .canonicalize()
        .context("failed to canonicalize child output directory")?;
    let section_path = child_session_dir
        .join("output")
        .join(file_path)
        .canonicalize()
        .context("failed to canonicalize return-packet file path")?;
    if !section_path.starts_with(&output_dir) {
        anyhow::bail!("return-packet file resolved outside child output directory");
    }

    Ok((
        packet,
        ReturnPacketRef {
            child_session_id: child_session_id.to_string(),
            section_path: section_path.to_string_lossy().to_string(),
        },
    ))
}

/// Result of attempting auto-seed-fork resolution.
pub(crate) struct AutoSeedResult {
    pub(crate) is_fork: bool,
    pub(crate) session_arg: Option<String>,
    pub(crate) is_auto_seed_fork: bool,
}

/// Try to auto-fork from a warm seed session if conditions allow.
///
/// Returns the (possibly updated) fork and session state.
pub(crate) fn try_auto_seed_fork(
    project_root: &Path,
    resolved_tool: &ToolName,
    config: Option<&ProjectConfig>,
    is_fork: bool,
    session_arg: Option<String>,
    ephemeral: bool,
) -> AutoSeedResult {
    if is_fork || session_arg.is_some() || ephemeral {
        return AutoSeedResult {
            is_fork,
            session_arg,
            is_auto_seed_fork: false,
        };
    }

    let auto_seed_enabled = config.map(|c| c.session.auto_seed_fork).unwrap_or(true);
    if !auto_seed_enabled {
        return AutoSeedResult {
            is_fork,
            session_arg,
            is_auto_seed_fork: false,
        };
    }

    let seed_max_age = config.map(|c| c.session.seed_max_age_secs).unwrap_or(86400);
    let current_git_head = csa_session::detect_git_head(project_root);
    let needs_native_fork = matches!(
        TransportFactory::fork_method_for_tool(resolved_tool.as_str()),
        ForkMethod::Native,
    );
    let seed_result = if needs_native_fork {
        csa_scheduler::find_seed_session_for_native_fork(
            project_root,
            resolved_tool.as_str(),
            seed_max_age,
            current_git_head.as_deref(),
        )
    } else {
        csa_scheduler::find_seed_session(
            project_root,
            resolved_tool.as_str(),
            seed_max_age,
            current_git_head.as_deref(),
        )
    };
    match seed_result {
        Ok(Some(seed)) => {
            info!(
                seed_session = %seed.session_id,
                tool = %seed.tool_name,
                "Auto fork-from-seed: warm session found"
            );
            AutoSeedResult {
                is_fork: true,
                session_arg: Some(seed.session_id),
                is_auto_seed_fork: true,
            }
        }
        Ok(None) => {
            debug!("No seed session available, cold start");
            AutoSeedResult {
                is_fork,
                session_arg,
                is_auto_seed_fork: false,
            }
        }
        Err(e) => {
            debug!(error = %e, "Seed session lookup failed, falling back to cold start");
            AutoSeedResult {
                is_fork,
                session_arg,
                is_auto_seed_fork: false,
            }
        }
    }
}

/// Pre-create a session with forked provider_session_id in tool state so that
/// `execute_with_session_and_meta` can resume ACP from the forked provider
/// session on the first turn. Only applies to native forks.
///
/// Returns `(pre_created_session_id, effective_session_arg)` if a session was
/// created, or `(None, existing_arg)` otherwise.
pub(crate) fn pre_create_native_fork_session(
    project_root: &Path,
    fork_res: &ForkResolution,
    current_tool: &ToolName,
    description: Option<&str>,
    effective_session_arg: Option<String>,
) -> Result<(Option<String>, Option<String>)> {
    if effective_session_arg.is_some() {
        return Ok((None, effective_session_arg));
    }

    let Some(ref new_provider_id) = fork_res.provider_session_id else {
        return Ok((None, effective_session_arg));
    };

    let fork_desc = description.map(String::from).unwrap_or_else(|| {
        format!(
            "fork of {}",
            fork_res
                .source_session_id
                .get(..8)
                .unwrap_or(&fork_res.source_session_id)
        )
    });
    let mut pre_session = csa_session::create_session(
        project_root,
        Some(&fork_desc),
        Some(&fork_res.source_session_id),
        Some(current_tool.as_str()),
    )?;
    pre_session.genealogy.fork_of_session_id = Some(fork_res.source_session_id.clone());
    pre_session.genealogy.fork_provider_session_id =
        fork_res.source_provider_session_id.clone();
    pre_session.tools.insert(
        current_tool.as_str().to_string(),
        csa_session::ToolState {
            provider_session_id: Some(new_provider_id.clone()),
            last_action_summary: String::new(),
            last_exit_code: 0,
            updated_at: chrono::Utc::now(),
            token_usage: None,
        },
    );
    csa_session::save_session(&pre_session)?;
    info!(
        session = %pre_session.meta_session_id,
        provider_session = %new_provider_id,
        "Pre-created session with forked provider session for ACP resume"
    );
    let sid = pre_session.meta_session_id.clone();
    Ok((Some(sid.clone()), Some(sid)))
}

/// Remove a pre-created fork session when execution fails or tool failover
/// occurs. Takes the session ID by `&mut Option` so it is consumed (set to
/// `None`) after cleanup, preventing double-delete on subsequent error paths.
pub(crate) fn cleanup_pre_created_fork_session(
    session_id: &mut Option<String>,
    project_root: &Path,
) {
    if let Some(sid) = session_id.take() {
        match csa_session::delete_session(project_root, &sid) {
            Ok(()) => {
                info!(session = %sid, "Cleaned up pre-created fork session after failure");
            }
            Err(e) => {
                warn!(session = %sid, error = %e, "Failed to clean up pre-created fork session");
            }
        }
    }
}
