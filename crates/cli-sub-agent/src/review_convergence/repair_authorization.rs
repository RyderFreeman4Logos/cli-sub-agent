//! Explicit repair-only orchestration for ledger-authorized consolidated handoffs.

use std::path::Path;

use anyhow::{Context, Result, bail};
use csa_config::{EffectiveModelCatalog, GlobalConfig, ProjectConfig};
use csa_process::ProviderTurnCompletion;
use csa_session::convergence::{
    AdmittedModelIdentity, ArtifactEvidenceRef, CampaignId, ConsolidatedRepairAuthorization,
    ConvergenceEvent, ConvergenceLedgerStore, CsaSessionId, EpochRecord, RepairBatchId,
    RepairIntent, SessionRelativeArtifactPath, Sha256Digest,
};
use csa_session::{get_session_dir, publish_session_output_artifact};

use crate::cli::ReviewArgs;
use crate::pipeline::SessionCreationMode;
use crate::run_resource_overrides::RunResourceOverrides;
use crate::startup_env::StartupSubtreeEnv;

use super::repair_lifecycle::{
    claim_repair_action, finish_failed_repair, reconcile_incomplete_repair_intent, repair_intent,
    validate_exact_repair_batches,
};
use super::repair_source::{SourceRepairOwner, capture_epoch, validate_repair_advance};
use super::runner::finalize_frozen_identity;

const REPAIR_ARTIFACT_FILE: &str = "consolidated-repair-handoff.json";
const REPAIR_ARTIFACT_PATH: &str = "output/consolidated-repair-handoff.json";

pub(crate) struct RepairOnlyContext<'a> {
    pub(crate) args: &'a ReviewArgs,
    pub(crate) project_root: &'a Path,
    pub(crate) project_config: Option<&'a ProjectConfig>,
    pub(crate) global_config: &'a GlobalConfig,
    pub(crate) model_catalog: &'a EffectiveModelCatalog,
    pub(crate) current_depth: u32,
    pub(crate) startup_env: &'a StartupSubtreeEnv,
}

impl<'a> RepairOnlyContext<'a> {
    pub(crate) fn new(
        args: &'a ReviewArgs,
        project_root: &'a Path,
        project_config: Option<&'a ProjectConfig>,
        global_config: &'a GlobalConfig,
        model_catalog: &'a EffectiveModelCatalog,
        current_depth: u32,
        startup_env: &'a StartupSubtreeEnv,
    ) -> Self {
        Self {
            args,
            project_root,
            project_config,
            global_config,
            model_catalog,
            current_depth,
            startup_env,
        }
    }
}

pub(crate) async fn run_repair_only_command(context: RepairOnlyContext<'_>) -> Result<i32> {
    let campaign_id = CampaignId::parse(
        context
            .args
            .campaign
            .as_deref()
            .context("--repair-only requires --campaign")?,
    )?;
    let store = ConvergenceLedgerStore::for_project(context.project_root)?;
    reconcile_incomplete_repair_intent(&store, context.project_root, &campaign_id)?;
    let authorization = store.authorize_consolidated_repairs_locked(&campaign_id)?;
    validate_clean_authorized_source(context.project_root, &authorization)?;

    let mut owner = SourceRepairOwner::acquire(context.project_root, authorization.epoch())?;
    validate_clean_authorized_source(context.project_root, &authorization)?;
    let claim = claim_repair_action(&store, &authorization)?;
    if let Err(error) = owner.bind_claim(&claim) {
        let release = owner.release();
        let error = match release {
            Ok(()) => error,
            Err(release_error) => error.context(format!(
                "source repair owner release also failed: {release_error:#}"
            )),
        };
        return finish_failed_repair(&store, &claim, error);
    }
    let intent = repair_intent(&authorization, claim.clone())?;
    if let Err(error) = store.persist_repair_intent(intent.clone()) {
        let release = owner.release();
        let error = match release {
            Ok(()) => anyhow::Error::from(error),
            Err(release_error) => anyhow::Error::from(error).context(format!(
                "source repair owner release also failed: {release_error:#}"
            )),
        };
        return finish_failed_repair(&store, &claim, error);
    }

    let result = execute_and_publish(&context, &store, &authorization, &intent).await;
    let release = owner.release();
    match (result, release) {
        (Ok(completion), Ok(())) => {
            if completion.completed_batches != intent.repair_batch_ids() {
                return finish_failed_repair(
                    &store,
                    &claim,
                    anyhow::anyhow!(
                        "repair completion did not contain the exact authorized batch IDs"
                    ),
                );
            }
            store
                .mark_repair_intent_committed(&claim, completion.new_epoch.clone())
                .map_err(anyhow::Error::from)?;
            store
                .finish_completion_action(&claim)
                .map_err(anyhow::Error::from)?;
            Ok(0)
        }
        (Ok(_), Err(error)) | (Err(error), Ok(())) => finish_failed_repair(&store, &claim, error),
        (Err(error), Err(release_error)) => finish_failed_repair(
            &store,
            &claim,
            error.context(format!(
                "source repair owner release also failed: {release_error:#}"
            )),
        ),
    }
}

fn validate_clean_authorized_source(
    project_root: &Path,
    authorization: &ConsolidatedRepairAuthorization,
) -> Result<()> {
    let observed = capture_epoch(project_root, authorization.epoch().base_oid())?;
    if !observed.clean {
        bail!("repair authorization requires an exactly clean index and worktree");
    }
    authorization.validate_observed_epoch(&observed.epoch)
}

async fn execute_and_publish(
    context: &RepairOnlyContext<'_>,
    store: &ConvergenceLedgerStore,
    authorization: &ConsolidatedRepairAuthorization,
    intent: &RepairIntent,
) -> Result<RepairCompletion> {
    validate_exact_repair_batches(authorization, intent)?;
    let selected = authorization
        .campaign()
        .command_authority()
        .ordered_admitted()
        .first()
        .context("captured repair authority has no executor")?
        .clone();
    let tool = tool_for_identity(&selected)?;
    let model_spec = model_spec_for_identity(&selected);
    let prompt = build_repair_prompt(authorization)?;
    let review_routing = crate::review_routing::detect_review_routing_metadata(
        context.project_root,
        context.project_config,
    );
    let idle_timeout_seconds = crate::pipeline::resolve_effective_idle_timeout_seconds(
        context.project_config,
        context.args.idle_timeout,
        context.args.timeout,
    );
    let initial_response_timeout_seconds =
        crate::pipeline::resolve_effective_initial_response_timeout_for_tool(
            context.project_config,
            context.args.initial_response_timeout,
            context.args.idle_timeout,
            context.args.timeout,
            tool.as_str(),
        );
    let stream_mode = crate::review_cmd::resolve_review_stream_mode(
        context.args.stream_stdout,
        context.args.no_stream_stdout,
    );
    let outcome = crate::review_cmd::execute::execute_review_with_tier_filter(
        tool,
        prompt,
        None,
        None,
        Some(model_spec.clone()),
        None,
        false,
        Vec::new(),
        None,
        format!(
            "authorized consolidated repair for {}",
            authorization.campaign().id()
        ),
        context.project_root,
        context.project_config,
        context.global_config,
        context.model_catalog,
        csa_hooks::load_global_pre_session_hook_invocation(),
        review_routing,
        stream_mode,
        idle_timeout_seconds,
        initial_response_timeout_seconds,
        false,
        authorization
            .campaign()
            .command_authority()
            .policy()
            .force_ignore(),
        true,
        None,
        context.args.build_jobs,
        context.args.fast_but_more_cost,
        false,
        false,
        false,
        false,
        &[],
        &[],
        context.args.error_marker_scan_override(),
        RunResourceOverrides::new(context.args.memory_max_mb, context.args.min_free_memory_mb),
        context.current_depth,
        SessionCreationMode::DaemonManaged,
        context.startup_env,
    )
    .await?;
    if outcome.execution.execution.exit_code != 0 || outcome.forced_decision.is_some() {
        bail!("authorized repair writer did not complete successfully");
    }
    if outcome.execution.execution.provider_turn_completion() != ProviderTurnCompletion::Natural {
        bail!("authorized repair writer provider turn was not naturally complete");
    }
    let actual_spec = outcome.routed_to.as_deref().unwrap_or(&model_spec);
    let actual_executor = finalize_frozen_identity(
        authorization.campaign().command_authority(),
        actual_spec,
        outcome.executed_tool.as_str(),
    )?;
    if actual_executor != selected {
        bail!("actual repair executor differs from the selected frozen identity");
    }

    let changed = capture_epoch(context.project_root, authorization.epoch().base_oid())?;
    validate_repair_advance(context.project_root, authorization.epoch(), &changed)?;
    let session_id = CsaSessionId::parse(&outcome.execution.meta_session_id)?;
    let artifact_bytes = encode_receipt(authorization, &actual_executor, &changed.epoch)?;
    let artifact_digest = Sha256Digest::compute(&artifact_bytes);
    let session_dir = get_session_dir(context.project_root, session_id.as_str())?;
    publish_session_output_artifact(&session_dir, REPAIR_ARTIFACT_FILE, &artifact_bytes)?;
    let artifact = ArtifactEvidenceRef::new(
        session_id,
        SessionRelativeArtifactPath::new(REPAIR_ARTIFACT_PATH)?,
        artifact_digest,
    );
    let mut events = intent
        .repair_batch_ids()
        .iter()
        .map(|batch_id| {
            authorization
                .handoff_for(batch_id, actual_executor.clone(), artifact.clone())
                .map(ConvergenceEvent::RepairHandoffRecorded)
        })
        .collect::<Result<Vec<_>>>()?;
    let new_epoch = changed.epoch;
    events.push(ConvergenceEvent::EpochOpened(new_epoch.clone()));
    store
        .append_batch(authorization.campaign().id().clone(), events)
        .map_err(anyhow::Error::new)
        .context("publish repair handoffs and changed-HEAD epoch")?;
    Ok(RepairCompletion {
        completed_batches: intent.repair_batch_ids().to_vec(),
        new_epoch,
    })
}

struct RepairCompletion {
    completed_batches: Vec<RepairBatchId>,
    new_epoch: EpochRecord,
}

fn build_repair_prompt(authorization: &ConsolidatedRepairAuthorization) -> Result<String> {
    let batches = serde_json::to_string_pretty(authorization.batches())?;
    Ok(format!(
        "Implement the complete consolidated repair authorization below. Modify only the project checkout, run the required regression tests, and create one commit containing the finished repair. Do not perform review discovery or launch another writer. The batch JSON is immutable and complete; do not omit corrections, regression tests, documentation/contracts, compatibility migrations, or sibling call sites.\nCampaign: {}\nEpoch: {}\nCommand authority digest: {}\nConsolidated batches:\n{}",
        authorization.campaign().id(),
        authorization.epoch().id(),
        authorization.campaign().command_authority_digest(),
        batches,
    ))
}

fn encode_receipt(
    authorization: &ConsolidatedRepairAuthorization,
    actual_executor: &AdmittedModelIdentity,
    changed_epoch: &EpochRecord,
) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(&serde_json::json!({
        "schema_version": 1,
        "kind": "consolidated_repair_handoff",
        "campaign_id": authorization.campaign().id(),
        "authorized_epoch_id": authorization.epoch().id(),
        "changed_epoch_id": changed_epoch.id(),
        "command_authority_digest": authorization.campaign().command_authority_digest(),
        "actual_executor": actual_executor,
        "repair_batches": authorization.batches(),
    }))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn tool_for_identity(identity: &AdmittedModelIdentity) -> Result<csa_core::types::ToolName> {
    use csa_core::types::ToolName;
    match identity.tool() {
        "gemini-cli" => Ok(ToolName::GeminiCli),
        "opencode" => Ok(ToolName::Opencode),
        "codex" => Ok(ToolName::Codex),
        "claude-code" => Ok(ToolName::ClaudeCode),
        "openai-compat" => Ok(ToolName::OpenaiCompat),
        "hermes" => Ok(ToolName::Hermes),
        "antigravity-cli" => Ok(ToolName::AntigravityCli),
        tool => bail!("unknown tool {tool} in frozen repair authority"),
    }
}

fn model_spec_for_identity(identity: &AdmittedModelIdentity) -> String {
    format!(
        "{}/{}/{}/{}",
        identity.tool(),
        identity.provider(),
        identity.model(),
        identity.reasoning()
    )
}
