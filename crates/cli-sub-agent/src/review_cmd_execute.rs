//! Review execution helpers extracted from `review_cmd.rs`.

use std::path::{Path, PathBuf};

use anyhow::Result;
use csa_config::{ExecutionEnvOptions, GlobalConfig, ProjectConfig};
use csa_core::types::{OutputFormat, ToolName};
use tracing::{info, warn};

use crate::review_routing::{ReviewRoutingMetadata, persist_review_routing_artifact};

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_review(
    tool: ToolName,
    prompt: String,
    session: Option<String>,
    model: Option<String>,
    tier_model_spec: Option<String>,
    tier_name: Option<String>,
    thinking: Option<String>,
    description: String,
    project_root: &Path,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    review_routing: ReviewRoutingMetadata,
    stream_mode: csa_process::StreamMode,
    idle_timeout_seconds: u64,
    initial_response_timeout_seconds: Option<u64>,
    force_override_user_config: bool,
    no_fs_sandbox: bool,
    readonly_project_root: bool,
    extra_writable: &[PathBuf],
) -> Result<crate::pipeline::SessionExecutionResult> {
    let enforce_tier = tier_model_spec.is_some();
    let executor = crate::pipeline::build_and_validate_executor(
        &tool,
        tier_model_spec.as_deref(),
        model.as_deref(),
        thinking.as_deref(),
        crate::pipeline::ConfigRefs {
            project: project_config,
            global: Some(global_config),
        },
        enforce_tier,
        force_override_user_config,
        false, // review must not inherit `csa run` per-tool defaults
    )
    .await?;

    let can_edit =
        project_config.is_none_or(|cfg| cfg.can_tool_edit_existing(executor.tool_name()));
    let can_write_new =
        project_config.is_none_or(|cfg| cfg.can_tool_write_new(executor.tool_name()));
    let effective_prompt = if !can_edit || !can_write_new {
        info!(
            tool = %executor.tool_name(),
            can_edit,
            can_write_new,
            "Applying filesystem restrictions via prompt injection"
        );
        executor.apply_restrictions(&prompt, can_edit, can_write_new)
    } else {
        prompt
    };

    let extra_env_owned = global_config.build_execution_env(
        executor.tool_name(),
        ExecutionEnvOptions::with_no_flash_fallback(),
    );
    let extra_env = extra_env_owned.as_ref();
    let _slot_guard = crate::pipeline::acquire_slot(&executor, global_config)?;

    if session.is_none()
        && let Ok(inherited_session_id) = std::env::var("CSA_SESSION_ID")
    {
        warn!(
            inherited_session_id = %inherited_session_id,
            "Ignoring inherited CSA_SESSION_ID for `csa review`; pass --session to resume explicitly"
        );
    }

    let execution = crate::pipeline::execute_with_session_and_meta_with_parent_source(
        &executor,
        &tool,
        &effective_prompt,
        OutputFormat::Json,
        session,
        Some(description),
        None,
        project_root,
        project_config,
        extra_env,
        Some("review"),
        tier_name.as_deref(),
        None,
        stream_mode,
        idle_timeout_seconds,
        initial_response_timeout_seconds,
        None,
        None,
        Some(global_config),
        crate::pipeline::ParentSessionSource::ExplicitOnly,
        no_fs_sandbox,
        readonly_project_root,
        extra_writable,
    )
    .await?;

    persist_review_routing_artifact(project_root, &execution.meta_session_id, &review_routing);

    Ok(execution)
}

/// Compute a SHA-256 content hash of the diff being reviewed.
///
/// The fingerprint enables diff-level deduplication: if two review
/// invocations produce the same diff content (e.g., revert-then-revert),
/// the second can reuse the first review's result.
pub(super) fn compute_diff_fingerprint(project_root: &Path, scope: &str) -> Option<String> {
    use sha2::{Digest, Sha256};

    let diff_args: Vec<&str> = if scope == "uncommitted" {
        vec!["diff", "HEAD"]
    } else if let Some(range) = scope.strip_prefix("range:") {
        vec!["diff", range]
    } else if let Some(base) = scope.strip_prefix("base:") {
        vec!["diff", base]
    } else {
        return None;
    };

    let output = std::process::Command::new("git")
        .args(&diff_args)
        .current_dir(project_root)
        .output()
        .ok()?;

    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }

    let digest = Sha256::digest(&output.stdout);
    Some(format!("sha256:{digest:x}"))
}
