use crate::cli::ReviewArgs;
use anyhow::Result;
use csa_config::ProjectConfig;
use csa_core::types::ReviewDecision;
use csa_session::{ReviewSessionMeta, ReviewVerdictArtifact, SessionArtifact};
use tracing::warn;

pub(super) fn load_prior_rounds_section_or_persist_error(
    args: &ReviewArgs,
    project_root: &std::path::Path,
    review_description: &str,
) -> Result<Option<String>> {
    match args
        .prior_rounds_summary
        .as_deref()
        .map(crate::review_prior_rounds::load_prior_rounds_section)
        .transpose()
    {
        Ok(section) => Ok(section),
        Err(err) => Err(crate::session_guard::persist_pre_exec_error_result(
            crate::session_guard::PreExecErrorCtx {
                project_root,
                session_id: review_pre_exec_session_id(args),
                description: Some(review_description),
                parent: None,
                tool_name: explicit_review_tool(args).map(|tool| tool.as_str()),
                task_type: Some("review"),
                tier_name: args.tier.as_deref(),
                error: err,
            },
        )),
    }
}

pub(super) fn review_pre_exec_session_id(args: &ReviewArgs) -> Option<&str> {
    args.session_id.as_deref().or(args.session.as_deref())
}

pub(super) fn explicit_review_tool(args: &ReviewArgs) -> Option<csa_core::types::ToolName> {
    args.tool.or_else(|| {
        args.model_spec
            .as_deref()
            .and_then(|spec| spec.split('/').next())
            .and_then(|tool_name| crate::run_helpers::parse_tool_name(tool_name).ok())
    })
}

pub(super) fn persist_tier_bypass_pre_exec_error(
    args: &ReviewArgs,
    project_root: &std::path::Path,
    effective_tier: Option<&str>,
    err: anyhow::Error,
) -> anyhow::Error {
    crate::session_guard::persist_pre_exec_error_result(crate::session_guard::PreExecErrorCtx {
        project_root,
        session_id: review_pre_exec_session_id(args),
        description: Some("review"),
        parent: None,
        tool_name: explicit_review_tool(args).map(|tool| tool.as_str()),
        task_type: Some("review"),
        tier_name: effective_tier,
        error: err,
    })
}

pub(super) fn validate_direct_tool_gate(
    args: &ReviewArgs,
    project_root: &std::path::Path,
    project_config: Option<&ProjectConfig>,
    effective_tier: Option<&str>,
    direct_tool_requested: bool,
) -> Result<()> {
    super::resolve::validate_review_direct_tool_tier_restriction(
        direct_tool_requested,
        project_config,
        effective_tier,
        args.force_override_user_config,
        args.force_ignore_tier_setting,
        args.model_spec.is_some(),
    )
    .map_err(|err| persist_daemon_review_pre_exec_error(args, project_root, effective_tier, err))
}

pub(super) fn persist_daemon_review_pre_exec_error(
    args: &ReviewArgs,
    project_root: &std::path::Path,
    effective_tier: Option<&str>,
    err: anyhow::Error,
) -> anyhow::Error {
    let Some(session_id) = args.session_id.as_deref() else {
        // Foreground review validation failures intentionally remain ordinary
        // CLI errors: there is no daemon placeholder session to complete.
        return err;
    };
    let error_message = err.to_string();
    let primary_failure = primary_failure_for_pre_exec_error(&error_message);
    let tool_name = explicit_review_tool(args)
        .map(|tool| tool.as_str().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let err = crate::session_guard::persist_pre_exec_error_result(
        crate::session_guard::PreExecErrorCtx {
            project_root,
            session_id: Some(session_id),
            description: Some("review"),
            parent: None,
            tool_name: Some(tool_name.as_str()),
            task_type: Some("review"),
            tier_name: effective_tier,
            error: err,
        },
    );
    persist_review_pre_exec_unavailable_sidecars(
        args,
        project_root,
        session_id,
        tool_name.as_str(),
        primary_failure,
        &error_message,
    );
    err
}

fn primary_failure_for_pre_exec_error(error_message: &str) -> &'static str {
    if error_message.contains("Direct --tool is restricted when tiers are configured") {
        "direct_tool_tier_restricted"
    } else {
        "review_pre_exec_error"
    }
}

fn persist_review_pre_exec_unavailable_sidecars(
    args: &ReviewArgs,
    project_root: &std::path::Path,
    session_id: &str,
    tool_name: &str,
    primary_failure: &str,
    error_message: &str,
) {
    let Ok(session_dir) = csa_session::get_session_dir(project_root, session_id) else {
        warn!(
            session_id,
            "Failed to resolve daemon review session dir for pre-exec sidecars"
        );
        return;
    };
    let failure_reason = format!("pre-exec: {error_message}");
    let meta = ReviewSessionMeta {
        session_id: session_id.to_string(),
        head_sha: csa_session::detect_git_head(project_root).unwrap_or_default(),
        decision: ReviewDecision::Unavailable.as_str().to_string(),
        verdict: crate::review_consensus::UNAVAILABLE.to_string(),
        review_mode: Some(args.effective_review_mode().as_str().to_string()),
        status_reason: Some(primary_failure.to_string()),
        routed_to: None,
        primary_failure: Some(primary_failure.to_string()),
        failure_reason: Some(failure_reason.clone()),
        tool: tool_name.to_string(),
        scope: super::resolve::derive_scope_for_project(args, project_root),
        exit_code: crate::verdict_exit_code::exit_code_from_review_decision(
            ReviewDecision::Unavailable,
        ),
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations: 1,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
        fix_convergence: None,
    };
    if let Err(err) = csa_session::write_review_meta(&session_dir, &meta) {
        warn!(session_id, error = %err, "Failed to write pre-exec review_meta.json");
    }
    let mut artifact = ReviewVerdictArtifact::from_parts(
        session_id.to_string(),
        ReviewDecision::Unavailable,
        crate::review_consensus::UNAVAILABLE,
        &[],
        Vec::new(),
    );
    artifact.primary_failure = meta.primary_failure.clone();
    artifact.failure_reason = meta.failure_reason.clone();
    artifact.review_mode = meta.review_mode.clone();
    match csa_session::write_review_verdict(&session_dir, &artifact) {
        Ok(()) => append_review_verdict_artifact_to_pre_exec_result(project_root, session_id),
        Err(err) => warn!(
            session_id,
            error = %err,
            "Failed to write pre-exec output/review-verdict.json"
        ),
    }
}

fn append_review_verdict_artifact_to_pre_exec_result(
    project_root: &std::path::Path,
    session_id: &str,
) {
    let Ok(Some(mut result)) = csa_session::load_result(project_root, session_id) else {
        return;
    };
    const REVIEW_VERDICT_ARTIFACT: &str = "output/review-verdict.json";
    if !result
        .artifacts
        .iter()
        .any(|artifact| artifact.path == REVIEW_VERDICT_ARTIFACT)
    {
        result
            .artifacts
            .push(SessionArtifact::new(REVIEW_VERDICT_ARTIFACT));
        if let Err(err) = csa_session::save_result(project_root, session_id, &result) {
            warn!(
                session_id,
                error = %err,
                "Failed to attach review verdict artifact to pre-exec result"
            );
        }
    }
}
