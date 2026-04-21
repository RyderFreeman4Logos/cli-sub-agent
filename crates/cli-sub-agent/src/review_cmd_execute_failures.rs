use super::*;

pub(super) fn classify_review_failover_reason(
    tool: ToolName,
    model_spec: Option<&str>,
    execution: &crate::pipeline::SessionExecutionResult,
    status_reason: Option<&str>,
) -> Option<String> {
    if status_reason == Some("gemini_auth_prompt") {
        return Some("gemini_auth_prompt".to_string());
    }

    classify_next_model_failure(
        tool.as_str(),
        &execution.execution.stderr_output,
        &execution.execution.output,
        execution.execution.exit_code,
        model_spec,
    )
    .map(|detected| detected.reason)
}

pub(super) fn build_gemini_api_key_retry_env(
    extra_env: Option<&HashMap<String, String>>,
) -> Option<HashMap<String, String>> {
    let env = extra_env?;
    if env.get(AUTH_MODE_ENV_KEY).map(String::as_str) != Some(AUTH_MODE_OAUTH) {
        return None;
    }

    let api_key = env.get(API_KEY_FALLBACK_ENV_KEY)?;
    let mut retry_env = env.clone();
    retry_env.insert(API_KEY_ENV.to_string(), api_key.clone());
    retry_env.insert(AUTH_MODE_ENV_KEY.to_string(), AUTH_MODE_API_KEY.to_string());
    retry_env.remove(API_KEY_FALLBACK_ENV_KEY);
    Some(retry_env)
}

pub(super) fn classify_review_failure_result(
    project_root: &Path,
    tool: ToolName,
    execution: &mut crate::pipeline::SessionExecutionResult,
    failure: ToolReviewFailureKind,
) -> Result<()> {
    let summary = failure.summary_note().to_string();
    fail_review_execution(project_root, tool, execution, &summary)
}

pub(super) fn maybe_synthesize_missing_review_result(
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
    let peak_memory_mb = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<PeakMemoryContext>())
        .and_then(|ctx| ctx.0);
    let fallback_result = SessionResult {
        status: status.to_string(),
        exit_code,
        summary,
        tool: tool.to_string(),
        started_at,
        completed_at,
        events_count: 0,
        artifacts: Vec::new(),
        peak_memory_mb,
        manager_fields: Default::default(),
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

pub(super) fn extract_meta_session_id_from_error(error: &anyhow::Error) -> Option<String> {
    const MARKER: &str = "meta_session_id=";
    for cause in error.chain() {
        let message = cause.to_string();
        let Some(idx) = message.find(MARKER) else {
            continue;
        };
        let suffix = &message[idx + MARKER.len()..];
        let session_id: String = suffix
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
            .collect();
        if !session_id.is_empty() {
            return Some(session_id);
        }
    }
    None
}

pub(super) fn read_review_failure_excerpt(session_dir: &Path) -> Option<String> {
    let stderr_path = session_dir.join("stderr.log");
    let mut buf = Vec::with_capacity(4096);
    let file = fs::File::open(stderr_path).ok()?;
    let _ = file.take(4096).read_to_end(&mut buf);
    let contents = String::from_utf8_lossy(&buf);
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

fn fail_review_execution(
    project_root: &Path,
    tool: ToolName,
    execution: &mut crate::pipeline::SessionExecutionResult,
    summary: &str,
) -> Result<()> {
    if execution.execution.stderr_output.is_empty() {
        execution.execution.stderr_output = summary.to_string();
    } else if !execution.execution.stderr_output.contains(summary) {
        if !execution.execution.stderr_output.ends_with('\n') {
            execution.execution.stderr_output.push('\n');
        }
        execution.execution.stderr_output.push_str(summary);
        execution.execution.stderr_output.push('\n');
    }
    execution.execution.exit_code = 1;
    execution.execution.summary = summary.to_string();

    rewrite_failed_review_state(project_root, tool, &execution.meta_session_id, summary)
}

fn rewrite_failed_review_state(
    project_root: &Path,
    tool: ToolName,
    session_id: &str,
    summary: &str,
) -> Result<()> {
    let Some(mut persisted_result) = load_result(project_root, session_id)
        .with_context(|| format!("failed to load result.toml for review session {session_id}"))?
    else {
        return Ok(());
    };
    persisted_result.status = SessionResult::status_from_exit_code(1);
    persisted_result.exit_code = 1;
    persisted_result.summary = summary.to_string();
    save_result(project_root, session_id, &persisted_result).with_context(|| {
        format!("failed to rewrite result.toml for review session {session_id}")
    })?;

    let mut session = load_session(project_root, session_id)
        .with_context(|| format!("failed to load session state for review session {session_id}"))?;
    if let Some(tool_state) = session.tools.get_mut(tool.as_str()) {
        tool_state.last_action_summary = summary.to_string();
        tool_state.last_exit_code = 1;
        tool_state.updated_at = chrono::Utc::now();
        save_session(&session).with_context(|| {
            format!("failed to rewrite session state for review session {session_id}")
        })?;
    }

    Ok(())
}

pub(super) fn enforce_review_artifact_contract(
    project_root: &Path,
    tool: &ToolName,
    execution_started_at: DateTime<Utc>,
    execution: Option<&mut crate::pipeline::SessionExecutionResult>,
    error: Option<&anyhow::Error>,
) -> Result<()> {
    let Some(leaked_paths) =
        detect_repo_root_review_artifact_violations(project_root, execution_started_at)?
    else {
        return Ok(());
    };

    let message = format!(
        "review artifact contract violation: review wrote artifacts outside $CSA_SESSION_DIR/output during this run: {}",
        leaked_paths.join(", ")
    );

    if let Some(execution) = execution {
        fail_review_execution(project_root, *tool, execution, &message)?;
        return Err(anyhow::anyhow!(message)
            .context(format!("meta_session_id={}", execution.meta_session_id)));
    }

    if let Some(session_id) = error.and_then(extract_meta_session_id_from_error) {
        rewrite_failed_review_state(project_root, *tool, &session_id, &message)?;
        return Err(anyhow::anyhow!(message).context(format!("meta_session_id={session_id}")));
    }

    Err(anyhow::anyhow!(message))
}

pub(super) fn repair_completed_review_restriction_result(
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
