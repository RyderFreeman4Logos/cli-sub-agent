use csa_core::types::ReviewDecision;
use csa_session::ReviewDiffSize;
use csa_session::state::ReviewSessionMeta;

use crate::review_consensus::HAS_ISSUES;

#[cfg(test)]
use super::execute;
use super::findings_toml::persist_review_findings_toml;
use super::output::{
    fail_closed_review_meta, persist_review_verdict_artifact, persisted_review_verdict_exit_code,
    review_meta_for_verdict_artifact,
};
#[cfg(test)]
use crate::review_routing::ReviewRoutingMetadata;
#[cfg(test)]
use anyhow::Result;
#[cfg(test)]
use csa_config::{GlobalConfig, ProjectConfig};
#[cfg(test)]
use csa_core::types::ToolName;

pub(super) fn review_decision_from_verdict(verdict: &str) -> ReviewDecision {
    match verdict {
        super::CLEAN => ReviewDecision::Pass,
        "SKIP" => ReviewDecision::Skip,
        "UNCERTAIN" => ReviewDecision::Uncertain,
        "UNAVAILABLE" => ReviewDecision::Unavailable,
        _ => ReviewDecision::Fail,
    }
}

pub(crate) fn should_run_fix_loop(fix_requested: bool, decision: ReviewDecision) -> bool {
    fix_requested && matches!(decision, ReviewDecision::Fail)
}

#[cfg(test)]
pub(crate) fn persist_review_sidecars_if_session_exists(
    project_root: &std::path::Path,
    meta: &ReviewSessionMeta,
    persistable_session_id: Option<&str>,
) -> Option<i32> {
    persist_review_sidecars_if_session_exists_with_diff_size(
        project_root,
        meta,
        persistable_session_id,
        None,
        None,
    )
}

pub(super) fn persist_review_sidecars_if_session_exists_with_diff_size(
    project_root: &std::path::Path,
    meta: &ReviewSessionMeta,
    persistable_session_id: Option<&str>,
    diff_size: Option<&ReviewDiffSize>,
    large_diff_warning: Option<super::diff_size::LargeDiffWarning>,
) -> Option<i32> {
    let persistable_session_id = persistable_session_id?;
    let effective_meta = fail_closed_review_meta(project_root, meta);

    super::diff_size::persist_review_meta_with_diff_report(
        project_root,
        &effective_meta,
        diff_size,
        large_diff_warning,
    );
    persist_review_findings_toml(project_root, &effective_meta);
    let worktree_mutation_findings =
        super::dirty_tree::append_repo_write_audit_finding(project_root, persistable_session_id);
    let effective_meta = if worktree_mutation_findings.is_empty() {
        effective_meta
    } else {
        review_meta_with_blocking_worktree_mutation(effective_meta)
    };
    let verdict_artifact = persist_review_verdict_artifact(
        project_root,
        &effective_meta,
        &worktree_mutation_findings,
        Vec::new(),
    )
    .map(|mut artifact| {
        super::diff_size::persist_review_verdict_diff_report(
            project_root,
            &effective_meta.session_id,
            &mut artifact,
            diff_size,
            large_diff_warning,
        );
        artifact
    });
    let final_meta = verdict_artifact
        .as_ref()
        .map(|artifact| review_meta_for_verdict_artifact(&effective_meta, artifact))
        .unwrap_or(effective_meta);
    super::diff_size::persist_review_meta_with_diff_report(
        project_root,
        &final_meta,
        diff_size,
        large_diff_warning,
    );
    let verdict_exit_code = verdict_artifact
        .as_ref()
        .map(|artifact| crate::verdict_exit_code::exit_code_from_review_decision(artifact.decision))
        .unwrap_or_else(|| {
            persisted_review_verdict_exit_code(project_root, persistable_session_id)
        });
    if verdict_exit_code == 0 {
        crate::review_gate::maybe_write_review_gate_marker(
            project_root,
            &final_meta.head_sha,
            persistable_session_id,
            &final_meta.scope,
            final_meta.review_mode.as_deref(),
        );
    } else {
        crate::review_gate::remove_review_gate_marker_for_head(
            project_root,
            &final_meta.head_sha,
            Some(persistable_session_id),
        );
    }
    Some(verdict_exit_code)
}

fn review_meta_with_blocking_worktree_mutation(mut meta: ReviewSessionMeta) -> ReviewSessionMeta {
    let decision = meta
        .decision
        .parse::<ReviewDecision>()
        .unwrap_or(ReviewDecision::Uncertain);
    if matches!(decision, ReviewDecision::Pass | ReviewDecision::Skip) {
        meta.decision = ReviewDecision::Fail.as_str().to_string();
        meta.verdict = HAS_ISSUES.to_string();
        meta.exit_code =
            crate::verdict_exit_code::exit_code_from_review_decision(ReviewDecision::Fail);
    }
    meta
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_review_for_tests(
    tool: ToolName,
    prompt: String,
    session: Option<String>,
    model: Option<String>,
    tier_model_spec: Option<String>,
    tier_name: Option<String>,
    tier_fallback_enabled: bool,
    thinking: Option<String>,
    description: String,
    project_root: &std::path::Path,
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
    extra_writable: &[std::path::PathBuf],
    extra_readable: &[std::path::PathBuf],
    error_marker_scan_override: Option<bool>,
) -> Result<execute::ReviewExecutionOutcome> {
    execute::execute_review(
        tool,
        prompt,
        session,
        model,
        tier_model_spec,
        tier_name,
        tier_fallback_enabled,
        thinking,
        description,
        project_root,
        project_config,
        global_config,
        None,
        review_routing,
        stream_mode,
        idle_timeout_seconds,
        initial_response_timeout_seconds,
        force_override_user_config,
        force_ignore_tier_setting,
        no_failover,
        no_fs_sandbox,
        readonly_project_root,
        extra_writable,
        extra_readable,
        error_marker_scan_override,
    )
    .await
}
