pub(super) mod bundle;
mod clean_room;
mod clustering;
#[cfg(test)]
mod completion_authorization;
mod production_clean_room_provider;
mod provider_command_authority;
// Slice 3A defines the deterministic core without changing legacy production dispatch.
#[allow(dead_code)]
mod completion;
#[allow(dead_code)]
mod completion_resume;
#[allow(dead_code)]
mod completion_types;
mod continuation;
mod coverage;
// Slice 3B will wire the strict completion-only contract and clean-room prompt.
#[allow(dead_code)]
mod discovery_contract;
#[allow(dead_code)]
mod discovery_prompt;
pub(super) mod engine;
mod gate_evidence;
mod output;
mod persistence;
mod recovery;
pub(super) mod repair_authorization;
pub(super) use repair_authorization::{
    RepairOnlyContext as RepairContext, run_repair_only_command as run_repair,
};
#[cfg(test)]
mod clean_room_provider_tests;
#[cfg(test)]
mod clean_room_tests;
#[cfg(test)]
mod completion_fresh_start_tests;
#[cfg(test)]
mod completion_resume_tests;
#[cfg(test)]
mod completion_tests;
#[cfg(test)]
mod gate_evidence_tests;
#[cfg(test)]
mod provider_command_authority_tests;
pub(super) mod runner;
mod schema;
pub(super) mod verification;
mod verification_runner;
mod verification_schema;

use anyhow::Result;
use csa_session::convergence::{
    AdmittedModelIdentity, CampaignId, CommandAuthorityCatalogIdentity, CommandAuthorityPolicy,
    CommandAuthoritySnapshot, CommandAuthoritySource, ConvergenceLedgerStore, Sha256Digest,
};

pub(super) struct EarlyCommandContext<'a> {
    pub(super) args: &'a crate::cli::ReviewArgs,
    pub(super) project_root: &'a std::path::Path,
    pub(super) project_config: Option<&'a csa_config::ProjectConfig>,
    pub(super) global_config: &'a csa_config::GlobalConfig,
    pub(super) model_catalog: &'a csa_config::EffectiveModelCatalog,
    pub(super) effective_tier: Option<&'a str>,
    pub(super) selection: &'a super::session_fix::SelectionToolResolution,
    pub(super) current_depth: u32,
    pub(super) startup_env: &'a crate::startup_env::StartupSubtreeEnv,
}

type EarlyCommandInputs<'a> = (
    &'a crate::cli::ReviewArgs,
    &'a std::path::Path,
    Option<&'a csa_config::ProjectConfig>,
    &'a csa_config::GlobalConfig,
    &'a csa_config::EffectiveModelCatalog,
    Option<&'a str>,
    &'a super::session_fix::SelectionToolResolution,
    u32,
    &'a crate::startup_env::StartupSubtreeEnv,
);

/// Run the legacy discovery path only after the caller has completed its common admission checks.
pub(super) async fn maybe_run_early_command(input: EarlyCommandInputs<'_>) -> Result<Option<i32>> {
    let (
        args,
        project_root,
        project_config,
        global_config,
        model_catalog,
        effective_tier,
        selection,
        current_depth,
        startup_env,
    ) = input;
    if !args.converge {
        return Ok(None);
    }
    run_early_command(EarlyCommandContext {
        args,
        project_root,
        project_config,
        global_config,
        model_catalog,
        effective_tier,
        selection,
        current_depth,
        startup_env,
    })
    .await
    .map(Some)
}

/// Emit the default convergence report without loading configuration or invoking a provider.
pub(super) fn emit_report_only(range: Option<&str>) -> Result<i32> {
    let range = range.ok_or_else(|| anyhow::anyhow!("validated convergence range is missing"))?;
    println!(
        "{}",
        serde_json::json!({
            "kind": "convergence_report_only",
            "range": range,
            "message": "report mode is read-only; use --converge --discovery-only for legacy discovery or --converge --execute-completion to request execution",
            "provider_calls": 0,
            "review_verdict": null,
            "merge_attestation": false,
        })
    );
    Ok(0)
}

/// Refuse an admitted execute request until clustered-resume ports are wired in a later slice.
///
/// This is deliberately before model selection, leases, provider calls, gates, or ledger writes.
pub(super) fn emit_completion_not_wired() -> Result<i32> {
    eprintln!(
        "{}",
        serde_json::json!({
            "kind": "convergence_completion_blocked",
            "reason_code": "completion_runtime_not_wired",
            "message": "completion execution was explicitly requested and admitted by policy, but clustered-resume execution is not wired yet",
            "provider_calls": 0,
            "review_verdict": null,
            "merge_attestation": false,
        })
    );
    Ok(1)
}

/// Admit the explicit capability before selecting a provider or creating execution state.
pub(super) fn ensure_completion_execution_is_allowed(
    global_policy: &csa_config::ConvergenceCompletionPolicy,
    project_policy: Option<&csa_config::ProjectConvergenceCompletionPolicy>,
) -> Result<()> {
    csa_config::ConvergenceCompletionPolicy::effective(global_policy, project_policy)
        .require_explicit_execution(true)
}

pub(super) async fn run_early_command(context: EarlyCommandContext<'_>) -> Result<i32> {
    let args = context.args;
    let Some(range_label) = args.range.as_deref() else {
        return Err(anyhow::anyhow!("validated convergence range is missing"));
    };
    let review_description = format!("convergence discovery observation for {range_label}");
    let detected_parent_tool = crate::run_helpers::detect_parent_tool();
    let parent_tool = crate::run_helpers::resolve_tool(detected_parent_tool, context.global_config);
    crate::run_helpers::warn_if_tier_without_tool(
        args.tier.as_deref(),
        context.selection.direct_tool_requested,
    );
    let resolved_selection = super::session_fix::resolve_selection_or_persist_error(
        super::session_fix::SelectionResolutionCtx {
            args,
            project_config: context.project_config,
            global_config: context.global_config,
            model_catalog: context.model_catalog,
            parent_tool: parent_tool.as_deref(),
            project_root: context.project_root,
            effective_tier: context.effective_tier,
            selection_tool: context.selection.selection_tool,
            direct_tool_requested: context.selection.direct_tool_requested,
            session_fix: context.selection.session_fix.as_ref(),
            review_description: &review_description,
        },
    )?;
    let tool = resolved_selection.tool;
    let resolved_model_spec = resolved_selection.model_spec;
    let tier_preference_order = resolved_selection.tier_preference_order;
    let tier_active = resolved_model_spec.is_some()
        && args.model_spec.is_none()
        && !args.force_ignore_tier_setting;
    let execution_no_failover = super::session_fix::effective_no_failover_for_session_fix(
        args.no_failover,
        context.selection.session_fix.as_ref(),
    );
    let resolved_tier_name = if tier_active {
        super::resolve_review_tier_name(
            context.project_config,
            context.global_config,
            context.effective_tier,
            args.force_override_user_config,
            args.force_ignore_tier_setting,
        )?
    } else {
        None
    };
    let config_review_model = context
        .project_config
        .and_then(|value| value.review.as_ref())
        .and_then(|value| value.model.as_deref())
        .or(context.global_config.review.model.as_deref());
    let review_model = super::resolve_review_model(
        args.model.as_deref(),
        config_review_model,
        resolved_model_spec.is_some(),
    );
    let review_thinking = super::resolve_review_thinking(
        args.thinking.as_deref(),
        context
            .project_config
            .and_then(|value| value.review.as_ref())
            .and_then(|value| value.thinking.as_deref())
            .or(context.global_config.review.thinking.as_deref()),
        resolved_model_spec.is_some(),
    );
    let stream_mode = super::resolve_review_stream_mode(args.stream_stdout, args.no_stream_stdout);
    let idle_timeout_seconds = crate::pipeline::resolve_effective_idle_timeout_seconds(
        context.project_config,
        args.idle_timeout,
        args.timeout,
    );
    let initial_response_timeout_seconds =
        super::resolve_effective_initial_response_timeout_for_tool(
            context.project_config,
            args.initial_response_timeout,
            args.idle_timeout,
            args.timeout,
            tool.as_str(),
        );
    let review_routing = crate::review_routing::detect_review_routing_metadata(
        context.project_root,
        context.project_config,
    );
    run_resolved_command(runner::ResolvedCommandContext {
        project_root: context.project_root,
        args,
        project_config: context.project_config,
        global_config: context.global_config,
        model_catalog: context.model_catalog,
        pre_session_hook: None,
        review_routing,
        tool,
        tier_model_spec: resolved_model_spec,
        tier_name: resolved_tier_name,
        tier_fallback_enabled: tier_active,
        tier_preference_order,
        model: review_model,
        thinking: review_thinking,
        stream_mode,
        idle_timeout_seconds,
        initial_response_timeout_seconds,
        no_failover: execution_no_failover,
        current_depth: context.current_depth,
        startup_env: context.startup_env,
    })
    .await
}

// Canonical JSON; scope and lens dimensions are lexically ordered.
const POLICY_JSON: &[u8] = br#"{"coverage_manifest":{"lenses":["correctness","resource_lifecycle","security"],"scope_kinds":["crate","domain","module"]},"kind":"convergence_discovery_observation","provider_call_budget_per_cell":4,"schema_version":1,"semantic_coverage":"scope_lens_manifest"}"#;

fn coverage_manifest_policy_digest() -> Sha256Digest {
    Sha256Digest::compute(POLICY_JSON)
}

pub(super) async fn run_command(
    context: runner::ResolvedCommandContext<'_>,
    range: &str,
) -> Result<i32> {
    let project_root = context.project_root;
    let command_authority = capture_command_authority(&context).await?;
    let input = engine::ObservationInput::new(range, command_authority.clone());
    let store = match ConvergenceLedgerStore::for_project(project_root) {
        Ok(store) => store,
        Err(error) => return emit_setup_block("store_failure", &error),
    };
    let exact_evidence = match bundle::build_exact_oid_evidence(project_root, range) {
        Ok(evidence) => evidence,
        Err(error) => return emit_setup_block("evidence_capture_failure", &error),
    };
    let (provider_evidence, provider_bundle) = match exact_evidence.publish(&store) {
        Ok(published) => published,
        Err(error) => return emit_setup_block("evidence_publish_failure", &error),
    };
    let runner_context = context.runner_context(provider_bundle, command_authority);
    let mut probe = runner::GitWorkspaceProbe::new(project_root, provider_evidence);
    let mut discovery_runner = runner::ProductionDiscoveryRunner::new(runner_context);
    match engine::run_discovery_observation(&input, &mut probe, &mut discovery_runner, &store).await
    {
        Ok(summary) => {
            let verification = async {
                let frozen = engine::WorkspaceProbe::capture(&mut probe, range)?;
                let epoch = frozen.epoch()?;
                if summary.epoch_id != epoch.id().as_str() {
                    anyhow::bail!(
                        "discovery summary epoch differs from the current immutable workspace epoch"
                    );
                }
                let campaign_id = CampaignId::parse(&summary.campaign_id)?;
                let ledger = store.load()?;
                let campaign = verification::campaign_epoch(&ledger, &campaign_id, &epoch)?;
                let mut verifier = verification_runner::ProductionVerificationRunner::new(
                    discovery_runner.into_context(),
                );
                verification::verify_campaign(&store, &campaign, &epoch, &frozen, &mut verifier)
                    .await
                    .map_err(anyhow::Error::from)?;
                clustering::cluster_verified_findings(
                    &store,
                    &campaign,
                    &epoch,
                    &frozen,
                    &mut verifier,
                )
                .await
                .map_err(anyhow::Error::from)
            }
            .await;
            match verification {
                Ok(clustering) => {
                    println!(
                        "{}",
                        serde_json::json!({
                            "kind": "convergence_clustering_complete",
                            "discovery": summary,
                            "root_clusters": clustering.root_clusters,
                            "repair_batches": clustering.repair_batches,
                            "blocking_candidates": clustering.blocking_candidates,
                            "review_verdict": null,
                            "merge_attestation": false,
                        })
                    );
                    Ok(0)
                }
                Err(error) => emit_setup_block("verification_failure", &error),
            }
        }
        Err(error) => {
            eprintln!("{}", serde_json::to_string(error.diagnostic())?);
            Ok(1)
        }
    }
}

async fn capture_command_authority(
    context: &runner::ResolvedCommandContext<'_>,
) -> Result<CommandAuthoritySnapshot> {
    let candidates = super::tier_candidates::review_ordered_tier_candidates(
        super::tier_candidates::ReviewTierCandidateRequest {
            initial_tool: context.tool,
            initial_model_spec: context.tier_model_spec.as_deref(),
            tier_name: context.tier_name.as_deref(),
            project_config: context.project_config,
            global_config: Some(context.global_config),
            model_catalog: context.model_catalog,
            tier_fallback_enabled: context.tier_fallback_enabled,
            no_failover: context.no_failover,
            tier_preference_order: &context.tier_preference_order,
        },
    )?;
    let mut admitted_identities = Vec::with_capacity(candidates.len());
    for (tool, model_spec) in &candidates {
        let admitted = crate::pipeline::build_and_validate_executor(
            tool,
            model_spec.as_deref(),
            context.model.as_deref(),
            context.thinking.as_deref(),
            crate::pipeline::ConfigRefs {
                project: context.project_config,
                global: Some(context.global_config),
                model_catalog: Some(context.model_catalog),
            },
            context.tier_name.is_some()
                && model_spec.is_some()
                && !context.args.force_ignore_tier_setting,
            context.args.force_override_user_config,
            false,
        )
        .await?;
        let spec = admitted.resolved_model_spec();
        let reasoning = match &spec.thinking_budget {
            csa_executor::ThinkingBudget::DefaultBudget => "default".to_string(),
            csa_executor::ThinkingBudget::Low => "low".to_string(),
            csa_executor::ThinkingBudget::Medium => "medium".to_string(),
            csa_executor::ThinkingBudget::High => "high".to_string(),
            csa_executor::ThinkingBudget::Xhigh => "xhigh".to_string(),
            csa_executor::ThinkingBudget::Max => "max".to_string(),
            csa_executor::ThinkingBudget::Custom(value) => value.to_string(),
        };
        admitted_identities.push(AdmittedModelIdentity::new(
            &spec.tool,
            &spec.provider,
            &spec.model,
            &reasoning,
        )?);
    }
    let source = if let Some(tier) = context.tier_name.as_deref() {
        CommandAuthoritySource::tier(tier, "review.tier")?
    } else if context.args.tool.is_some()
        || context.args.model_spec.is_some()
        || context.args.model.is_some()
    {
        CommandAuthoritySource::direct("review.command")?
    } else {
        CommandAuthoritySource::default_model("review.default")?
    };
    let catalog_version =
        Sha256Digest::compute(&serde_json::to_vec(&admitted_identities)?).to_string();
    CommandAuthoritySnapshot::new(
        source,
        CommandAuthorityPolicy::new(
            context.tier_fallback_enabled && !context.no_failover,
            context.tier_preference_order.clone(),
            context.args.force_ignore_tier_setting,
            context.no_failover,
        )?,
        CommandAuthorityCatalogIdentity::new("effective command catalog", &catalog_version)?,
        admitted_identities,
    )
}

pub(super) fn emit_setup_block(reason_code: &'static str, error: &anyhow::Error) -> Result<i32> {
    eprintln!(
        "{}",
        serde_json::json!({
            "kind": "convergence_discovery_blocked",
            "reason_code": reason_code,
            "message": format!("{error:#}"),
            "provider_calls": 0,
            "discovery_evidence_complete": false,
            "review_verdict": null,
            "merge_attestation": false
        })
    );
    Ok(1)
}

/// Emit an execute-mode setup failure without labelling it as a discovery report.
pub(super) fn emit_completion_setup_block(
    reason_code: &'static str,
    error: &anyhow::Error,
) -> Result<i32> {
    eprintln!(
        "{}",
        serde_json::json!({
            "kind": "convergence_completion_blocked",
            "reason_code": reason_code,
            "message": format!("{error:#}"),
            "provider_calls": 0,
            "review_verdict": null,
            "merge_attestation": false,
        })
    );
    Ok(1)
}

pub(super) async fn run_resolved_command(
    context: runner::ResolvedCommandContext<'_>,
) -> Result<i32> {
    let Some(range) = context.args.range.clone() else {
        return emit_setup_block(
            "invalid_convergence_input",
            &anyhow::anyhow!("validated convergence range is missing"),
        );
    };
    run_command(context, &range).await
}

#[cfg(test)]
mod tests;
