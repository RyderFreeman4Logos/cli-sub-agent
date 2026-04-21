use csa_core::types::ReviewDecision;
use csa_session::state::ReviewSessionMeta;

#[cfg(test)]
use super::execute;
use super::findings_toml::persist_review_findings_toml;
use super::output::{persist_review_meta, persist_review_verdict};
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

pub(crate) fn persist_review_sidecars_if_session_exists(
    project_root: &std::path::Path,
    meta: &ReviewSessionMeta,
    persistable_session_id: Option<&str>,
) {
    if persistable_session_id.is_none() {
        return;
    }

    persist_review_meta(project_root, meta);
    persist_review_findings_toml(project_root, meta);
    persist_review_verdict(project_root, meta, &[], Vec::new());
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
    )
    .await
}
