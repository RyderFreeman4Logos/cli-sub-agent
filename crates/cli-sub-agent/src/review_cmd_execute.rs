//! Review execution helpers extracted from `review_cmd.rs`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use csa_config::{ExecutionEnvOptions, GlobalConfig, ProjectConfig};
use csa_core::types::{OutputFormat, ToolName};
use csa_session::{SessionResult, load_result, load_session, save_result, save_session};
use tracing::{info, warn};

use crate::review_routing::{ReviewRoutingMetadata, persist_review_routing_artifact};

use super::output::{
    derive_review_result_summary, has_structured_review_content, is_edit_restriction_summary,
    is_review_output_empty,
};

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
    force_ignore_tier_setting: bool,
    no_fs_sandbox: bool,
    readonly_project_root: bool,
    extra_writable: &[PathBuf],
) -> Result<crate::pipeline::SessionExecutionResult> {
    let enforce_tier = tier_model_spec.is_some() && !force_ignore_tier_setting;
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

    let mut execution = crate::pipeline::execute_with_session_and_meta_with_parent_source(
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
    repair_completed_review_restriction_result(project_root, tool, &mut execution)?;

    Ok(execution)
}

fn repair_completed_review_restriction_result(
    project_root: &Path,
    tool: ToolName,
    execution: &mut crate::pipeline::SessionExecutionResult,
) -> Result<()> {
    if !should_repair_completed_review_restriction(&execution.execution) {
        return Ok(());
    }

    let repaired_summary = derive_review_result_summary(&execution.execution.output)
        .unwrap_or_else(|| execution.execution.summary.clone());

    info!(
        session_id = %execution.meta_session_id,
        tool = %tool,
        "Reclassifying completed review with edit restriction as success"
    );

    execution.execution.exit_code = 0;
    execution.execution.summary = repaired_summary.clone();

    let Some(mut persisted_result) = load_result(project_root, &execution.meta_session_id)
        .with_context(|| {
            format!(
                "failed to load result.toml for review session {}",
                execution.meta_session_id
            )
        })?
    else {
        return Ok(());
    };
    persisted_result.status = SessionResult::status_from_exit_code(0);
    persisted_result.exit_code = 0;
    persisted_result.summary = repaired_summary.clone();
    save_result(project_root, &execution.meta_session_id, &persisted_result).with_context(
        || {
            format!(
                "failed to rewrite repaired result.toml for review session {}",
                execution.meta_session_id
            )
        },
    )?;

    let mut session =
        load_session(project_root, &execution.meta_session_id).with_context(|| {
            format!(
                "failed to load session state for repaired review session {}",
                execution.meta_session_id
            )
        })?;
    if let Some(tool_state) = session.tools.get_mut(tool.as_str()) {
        tool_state.last_action_summary = repaired_summary;
        tool_state.last_exit_code = 0;
        tool_state.updated_at = chrono::Utc::now();
        save_session(&session).with_context(|| {
            format!(
                "failed to rewrite session state for repaired review session {}",
                execution.meta_session_id
            )
        })?;
    }

    Ok(())
}

fn should_repair_completed_review_restriction(execution: &csa_process::ExecutionResult) -> bool {
    execution.exit_code != 0
        && is_edit_restriction_summary(&execution.summary)
        && !is_review_output_empty(&execution.output)
        && has_structured_review_content(&execution.output)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review_cmd::tests::{
        ScopedEnvVarRestore, project_config_with_enabled_tools, setup_git_repo,
    };
    use crate::test_session_sandbox::ScopedSessionSandbox;
    use csa_config::{GlobalConfig, ProjectProfile, ToolRestrictions};
    use csa_core::types::ToolName;

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_review_reclassifies_complete_review_after_edit_restriction() {
        use std::os::unix::fs::PermissionsExt;

        let project_dir = setup_git_repo();
        let _sandbox = ScopedSessionSandbox::new(&project_dir);
        let bin_dir = project_dir.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let fake_opencode = bin_dir.join("opencode");
        std::fs::write(
            &fake_opencode,
            "#!/bin/sh\n\
printf '%s\\n' \
'<!-- CSA:SECTION:summary -->' \
'Review completed successfully.' \
'<!-- CSA:SECTION:summary:END -->' \
'' \
'<!-- CSA:SECTION:details -->' \
'Detailed review body.' \
'<!-- CSA:SECTION:details:END -->' \
'' \
'PASS'\n\
printf 'tool mutation\\n' >> tracked.txt\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&fake_opencode).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&fake_opencode, perms).unwrap();

        let inherited_path = std::env::var("PATH").unwrap_or_default();
        let patched_path = format!("{}:{inherited_path}", bin_dir.display());
        let _path_guard = ScopedEnvVarRestore::set("PATH", &patched_path);

        let mut config = project_config_with_enabled_tools(&["opencode"]);
        config.tools.get_mut("opencode").unwrap().restrictions = Some(ToolRestrictions {
            allow_edit_existing_files: false,
            allow_write_new_files: false,
        });

        let global = GlobalConfig::default();
        let result = execute_review(
            ToolName::Opencode,
            "scope=uncommitted mode=review-only security=auto".to_string(),
            None,
            None,
            None, // tier_model_spec
            None, // tier_name
            None, // thinking
            "review: edit-restriction-regression".to_string(),
            project_dir.path(),
            Some(&config),
            &global,
            ReviewRoutingMetadata {
                project_profile: ProjectProfile::Unknown,
                detection_method: "auto",
            },
            csa_process::StreamMode::BufferOnly,
            crate::pipeline::DEFAULT_IDLE_TIMEOUT_SECONDS,
            None,  // initial_response_timeout_seconds
            false, // force_override_user_config
            false, // force_ignore_tier_setting
            false, // no_fs_sandbox
            false, // readonly_project_root
            &[],   // extra_writable
        )
        .await
        .expect("review should succeed after reclassifying edit restriction");

        assert_eq!(result.execution.exit_code, 0);
        assert_eq!(result.execution.summary, "Review completed successfully.");

        let persisted = csa_session::load_result(project_dir.path(), &result.meta_session_id)
            .unwrap()
            .expect("result.toml");
        assert_eq!(persisted.status, "success");
        assert_eq!(persisted.exit_code, 0);
        assert_eq!(persisted.summary, "Review completed successfully.");

        let session =
            csa_session::load_session(project_dir.path(), &result.meta_session_id).unwrap();
        let tool_state = session.tools.get("opencode").expect("opencode tool state");
        assert_eq!(tool_state.last_exit_code, 0);
        assert_eq!(
            tool_state.last_action_summary,
            "Review completed successfully."
        );

        assert_eq!(
            std::fs::read_to_string(project_dir.path().join("tracked.txt")).unwrap(),
            "baseline\n"
        );
    }
}
