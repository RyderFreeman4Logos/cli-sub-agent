//! Explicit repair-only orchestration for ledger-authorized consolidated handoffs.

use std::path::Path;
use std::process::{Command, Output};

use anyhow::{Context, Result, bail};
use csa_config::{EffectiveModelCatalog, GlobalConfig, ProjectConfig};
use csa_process::ProviderTurnCompletion;
use csa_session::convergence::{
    AdmittedModelIdentity, ArtifactEvidenceRef, CampaignId, ConsolidatedRepairAuthorization,
    ConvergenceEvent, ConvergenceLedgerStore, CsaSessionId, EpochRecord, GitObjectId,
    SessionRelativeArtifactPath, Sha256Digest,
};
use csa_session::{get_session_dir, publish_session_output_artifact};

use crate::cli::ReviewArgs;
use crate::pipeline::SessionCreationMode;
use crate::run_resource_overrides::RunResourceOverrides;
use crate::startup_env::StartupSubtreeEnv;

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
    let authorization = store.authorize_consolidated_repairs_locked(&campaign_id)?;
    let observed = capture_epoch(context.project_root, authorization.epoch().base_oid())?;
    if !observed.clean {
        bail!("repair authorization requires an exactly clean index and worktree");
    }
    authorization.validate_observed_epoch(&observed.epoch)?;

    let original_head = authorization.epoch().head_oid().clone();
    let result = execute_and_publish(&context, &store, authorization).await;
    if let Err(error) = result {
        rollback_source(context.project_root, &original_head)
            .context("repair failed and source rollback also failed")?;
        return Err(error);
    }
    result
}

async fn execute_and_publish(
    context: &RepairOnlyContext<'_>,
    store: &ConvergenceLedgerStore,
    authorization: ConsolidatedRepairAuthorization,
) -> Result<i32> {
    let selected = authorization
        .campaign()
        .command_authority()
        .ordered_admitted()
        .first()
        .context("captured repair authority has no executor")?
        .clone();
    let tool = tool_for_identity(&selected)?;
    let model_spec = model_spec_for_identity(&selected);
    let prompt = build_repair_prompt(&authorization)?;
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
        RunResourceOverrides {
            memory_max_mb: context.args.memory_max_mb,
            min_free_memory_mb: context.args.min_free_memory_mb,
        },
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
    if !changed.clean || changed.epoch.head_oid() == authorization.epoch().head_oid() {
        bail!("authorized repair must create a changed clean HEAD before handoff publication");
    }
    let session_id = CsaSessionId::parse(&outcome.execution.meta_session_id)?;
    let artifact_bytes = encode_receipt(&authorization, &actual_executor, &changed.epoch)?;
    let artifact_digest = Sha256Digest::compute(&artifact_bytes);
    let session_dir = get_session_dir(context.project_root, session_id.as_str())?;
    publish_session_output_artifact(&session_dir, REPAIR_ARTIFACT_FILE, &artifact_bytes)?;
    let artifact = ArtifactEvidenceRef::new(
        session_id,
        SessionRelativeArtifactPath::new(REPAIR_ARTIFACT_PATH)?,
        artifact_digest,
    );
    let mut events = authorization
        .batches()
        .iter()
        .map(|batch| {
            authorization
                .handoff_for(batch.id(), actual_executor.clone(), artifact.clone())
                .map(ConvergenceEvent::RepairHandoffRecorded)
        })
        .collect::<Result<Vec<_>>>()?;
    events.push(ConvergenceEvent::EpochOpened(changed.epoch));
    store
        .append_batch(authorization.campaign().id().clone(), events)
        .map_err(anyhow::Error::new)
        .context("publish repair handoffs and changed-HEAD epoch")?;
    Ok(0)
}

struct CapturedEpoch {
    epoch: EpochRecord,
    clean: bool,
}

fn capture_epoch(project_root: &Path, base_oid: &GitObjectId) -> Result<CapturedEpoch> {
    let head = git(project_root, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    let head_oid = String::from_utf8(head.stdout)
        .context("repair HEAD was not UTF-8")?
        .trim()
        .to_owned();
    let diff = git(
        project_root,
        &[
            "diff",
            "--binary",
            "--full-index",
            "--no-ext-diff",
            base_oid.as_str(),
            &head_oid,
            "--",
        ],
    )?;
    let status = git(
        project_root,
        &["status", "--porcelain=v1", "--untracked-files=normal"],
    )?;
    Ok(CapturedEpoch {
        epoch: EpochRecord::new(
            base_oid.clone(),
            GitObjectId::parse(&head_oid)?,
            Sha256Digest::compute(&diff.stdout),
        ),
        clean: status.stdout.is_empty(),
    })
}

fn git(project_root: &Path, args: &[&str]) -> Result<Output> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .output()
        .with_context(|| format!("run git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output)
}

fn rollback_source(project_root: &Path, original_head: &GitObjectId) -> Result<()> {
    git(project_root, &["reset", "--hard", original_head.as_str()])?;
    git(project_root, &["clean", "-fd"])?;
    let captured = capture_epoch(project_root, original_head)?;
    if !captured.clean || captured.epoch.head_oid() != original_head {
        bail!("repair source rollback did not restore the original clean HEAD");
    }
    Ok(())
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
