//! Review execution helpers extracted from `review_cmd.rs`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use csa_config::{ExecutionEnvOptions, GlobalConfig, ProjectConfig};
use csa_core::types::{OutputFormat, ToolName};
use csa_session::{
    SessionResult, get_session_dir, load_result, load_session, save_result, save_session,
};
use tracing::{info, warn};

use crate::review_routing::{ReviewRoutingMetadata, persist_review_routing_artifact};

use super::output::{
    derive_review_result_summary, has_structured_review_content, is_edit_restriction_summary,
    is_review_output_empty,
};

fn review_execution_env_options(no_failover: bool) -> ExecutionEnvOptions {
    let options = ExecutionEnvOptions::with_no_flash_fallback();
    if no_failover {
        options.with_no_failover()
    } else {
        options
    }
}

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
    no_failover: bool,
    no_fs_sandbox: bool,
    readonly_project_root: bool,
    extra_writable: &[PathBuf],
) -> Result<crate::pipeline::SessionExecutionResult> {
    let execution_started_at = Utc::now();
    let enforce_tier =
        tier_name.is_some() && tier_model_spec.is_some() && !force_ignore_tier_setting;
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
    let mut effective_prompt = if !can_edit || !can_write_new {
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
        review_execution_env_options(no_failover),
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

    if let Some(guard) = crate::pipeline::prompt_guard::anti_recursion_guard(project_config) {
        effective_prompt = format!("{guard}\n\n{effective_prompt}");
    }

    let mut execution = match crate::pipeline::execute_with_session_and_meta_with_parent_source(
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
        crate::pipeline::SessionCreationMode::DaemonManaged,
        no_fs_sandbox,
        readonly_project_root,
        extra_writable,
    )
    .await
    {
        Ok(execution) => execution,
        Err(err) => {
            maybe_synthesize_missing_review_result(project_root, tool, execution_started_at, &err);
            return Err(err);
        }
    };

    persist_review_routing_artifact(project_root, &execution.meta_session_id, &review_routing);
    repair_completed_review_restriction_result(project_root, tool, &mut execution)?;

    Ok(execution)
}

fn maybe_synthesize_missing_review_result(
    project_root: &Path,
    tool: ToolName,
    started_at: DateTime<Utc>,
    error: &anyhow::Error,
) {
    let Some(session_id) = extract_meta_session_id_from_error(error) else {
        return;
    };

    match load_result(project_root, &session_id) {
        Ok(Some(_)) => return,
        Ok(None) => {}
        Err(load_err) => {
            warn!(
                session_id = %session_id,
                error = %load_err,
                "Failed to check for existing review result.toml before fallback synthesis"
            );
        }
    }

    let session_dir = match get_session_dir(project_root, &session_id) {
        Ok(path) => path,
        Err(session_dir_err) => {
            warn!(
                session_id = %session_id,
                error = %session_dir_err,
                "Failed to resolve review session dir for fallback result synthesis"
            );
            return;
        }
    };

    let stderr_excerpt = read_review_failure_excerpt(&session_dir)
        .unwrap_or_else(|| truncate_for_summary(&format!("{error:#}"), 500));
    let (status, exit_code, error_kind) = classify_review_failure(error, &stderr_excerpt);
    let summary = truncate_for_summary(
        &format!("review {error_kind}: {}", stderr_excerpt.trim()),
        200,
    );
    let completed_at = Utc::now();
    let fallback_result = SessionResult {
        status: status.to_string(),
        exit_code,
        summary,
        tool: tool.to_string(),
        started_at,
        completed_at,
        events_count: 0,
        artifacts: Vec::new(),
        peak_memory_mb: None,
    };

    if let Err(save_err) = save_result(project_root, &session_id, &fallback_result) {
        warn!(
            session_id = %session_id,
            error = %save_err,
            "Failed to synthesize missing review result.toml"
        );
        return;
    }

    csa_session::write_cooldown_marker_for_project(project_root, &session_id, completed_at);
    warn!(
        session_id = %session_id,
        error_kind,
        "Synthesized missing review result.toml after execution error"
    );
}

fn extract_meta_session_id_from_error(error: &anyhow::Error) -> Option<String> {
    const MARKER: &str = "meta_session_id=";
    for cause in error.chain() {
        let message = cause.to_string();
        let Some(idx) = message.find(MARKER) else {
            continue;
        };
        let suffix = &message[idx + MARKER.len()..];
        let session_id: String = suffix
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric())
            .collect();
        if !session_id.is_empty() {
            return Some(session_id);
        }
    }
    None
}

fn read_review_failure_excerpt(session_dir: &Path) -> Option<String> {
    let stderr_path = session_dir.join("stderr.log");
    let contents = fs::read_to_string(stderr_path).ok()?;
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(truncate_for_summary(trimmed, 500))
}

fn classify_review_failure(
    error: &anyhow::Error,
    excerpt: &str,
) -> (&'static str, i32, &'static str) {
    let mut combined = excerpt.to_ascii_lowercase();
    for cause in error.chain() {
        combined.push('\n');
        combined.push_str(&cause.to_string().to_ascii_lowercase());
    }

    if combined.contains("initial_response_timeout")
        || combined.contains("timed out")
        || combined.contains("timeout")
    {
        ("timeout", 124, "timeout")
    } else if combined.contains("sigkill")
        || combined.contains("sigterm")
        || combined.contains("killed")
        || combined.contains("terminated by signal")
    {
        ("signal", 137, "signal")
    } else if combined.contains("fork")
        || combined.contains("spawn")
        || combined.contains("provider_session_id")
    {
        ("failure", 1, "spawn_fail")
    } else {
        ("failure", 1, "tool_crash")
    }
}

fn truncate_for_summary(text: &str, max_chars: usize) -> String {
    let truncated: String = text.chars().take(max_chars).collect();
    truncated.trim().replace('\n', " ")
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
    use crate::session_cmds_result::{StructuredOutputOpts, handle_session_result};
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
            false, // no_failover
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

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_review_model_spec_bypasses_tier_enforcement_without_active_tier() {
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
'Explicit model spec review succeeded.' \
'<!-- CSA:SECTION:summary:END -->' \
'' \
'<!-- CSA:SECTION:details -->' \
'Explicit model spec bypassed tier enforcement.' \
'<!-- CSA:SECTION:details:END -->' \
'' \
'PASS'\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&fake_opencode).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&fake_opencode, perms).unwrap();

        let inherited_path = std::env::var("PATH").unwrap_or_default();
        let patched_path = format!("{}:{inherited_path}", bin_dir.display());
        let _path_guard = ScopedEnvVarRestore::set("PATH", &patched_path);

        let mut config = project_config_with_enabled_tools(&["opencode", "gemini-cli"]);
        config.tiers.insert(
            "quality".to_string(),
            csa_config::config::TierConfig {
                description: "Test tier".to_string(),
                models: vec!["gemini-cli/google/default/xhigh".to_string()],
                strategy: csa_config::TierStrategy::default(),
                token_budget: None,
                max_turns: None,
            },
        );

        let global = GlobalConfig::default();
        let result = execute_review(
            ToolName::Opencode,
            "scope=uncommitted mode=review-only security=auto".to_string(),
            None,
            None,
            Some("opencode/provider/model/medium".to_string()),
            None, // tier_name
            None, // thinking
            "review: model-spec-bypasses-tier-regression".to_string(),
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
            false, // no_failover
            false, // no_fs_sandbox
            false, // readonly_project_root
            &[],   // extra_writable
        )
        .await
        .expect("explicit review model spec should bypass tier enforcement");

        assert_eq!(result.execution.exit_code, 0);
        assert!(
            result
                .execution
                .output
                .contains("Explicit model spec review succeeded."),
            "expected structured review output, got: {}",
            result.execution.output
        );
    }

    #[test]
    fn synthesize_missing_review_result_makes_session_result_readable() {
        let td = tempfile::tempdir().unwrap();
        let _sandbox = ScopedSessionSandbox::new(&td);
        let project_root = td.path();

        let session =
            csa_session::create_session(project_root, Some("review-failure"), None, Some("codex"))
                .expect("session creation");
        let session_id = session.meta_session_id.clone();
        let session_dir = csa_session::get_session_dir(project_root, &session_id).unwrap();
        std::fs::write(
            session_dir.join("stderr.log"),
            "codex daemon fork-from failed: provider session bootstrap failed",
        )
        .unwrap();

        let started_at = Utc::now();
        let err = anyhow::anyhow!("codex daemon fork-from failed")
            .context(format!("meta_session_id={session_id}"));

        maybe_synthesize_missing_review_result(project_root, ToolName::Codex, started_at, &err);

        let result = csa_session::load_result(project_root, &session.meta_session_id)
            .unwrap()
            .expect("synthetic result.toml should exist");
        assert_eq!(result.status, "failure");
        assert_eq!(result.exit_code, 1);
        assert!(
            result.summary.contains("spawn_fail"),
            "expected classified summary, got: {}",
            result.summary
        );

        handle_session_result(
            session.meta_session_id,
            false,
            Some(project_root.to_string_lossy().into_owned()),
            StructuredOutputOpts::default(),
        )
        .expect("session result should read the synthetic review failure");
    }
}
