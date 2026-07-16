//! Production ports for the explicit clustered-completion command.
//!
//! The reducer remains side-effect free. This adapter owns every filesystem lease and performs
//! host-only work without a provider reservation; it reserves and reconciles the one clean-room
//! provider turn before it can send the provider request.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use csa_session::convergence::{
    ArtifactEvidenceRef, CampaignId, CleanRoomReviewArtifactBindings, CleanRoomReviewRecord,
    CommandAuthoritySnapshot, CompletionActionId, CompletionActionJournalRead,
    ConvergenceLedgerStore, CsaSessionId, GateCommandResult, GateEvidenceRecord, ModelEvidence,
    ObservedToolEvidence, ProviderTurnExecutionId, ProviderTurnReservation,
    SessionRelativeArtifactPath,
};

use super::bundle;
use super::clean_room::{
    CleanupFailureLedger, CurrentCheckoutCleanup, DetachedWorkspaceLease,
    acquire_current_checkout_lease,
};
use super::clean_room_provider::{
    ExactProviderPrompt, ProviderSessionFactory, ProviderSessionRequest,
};
use super::clean_room_v2::{HostReviewArtifactStore, ReviewEnvelopeContext};
use super::completion::{CompletionPorts, ProviderTurnAllowance};
use super::completion_authorization::CompletionAuthorizationEvent;
use super::completion_types::{
    CompletionAction, CompletionEvent, CompletionExecutionReservation, CompletionPortError,
    CompletionPortResult, ProviderTurnReconciliation,
};
use super::discovery_prompt::build_clean_room_prompt;
use super::gate_authority::{
    FinalGateAuthority, FinalGatePlan, GateCommandAuthority, GateNetworkPolicy,
};
use super::gate_evidence::{HostFinalGatePort, HostGateArtifactStore};
use super::production_clean_room_provider::ProductionCleanRoomProvider;
use super::production_completion_gate::BlockingDirectFinalGateDriver;
use super::production_completion_support::{
    allowed_provider_environment, campaign_record, reconciliation_from_completion,
};
use super::provider_command_authority::{
    ProviderCommandAuthority, ProviderEnvironmentInputs, SystemProviderProgramResolver,
};
use super::runner::ResolvedCommandContext;

const COMPLETION_GATE_AUTHORITY_VERSION: &str = "linux-x86_64-v1";
const COMPLETION_GATE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const COMPLETION_LEASE_TIMEOUT: Duration = Duration::from_secs(30);
const COMPLETION_MAX_CYCLES: u32 = 32;
const COMPLETION_MAX_PROVIDER_TURNS: u32 = 16;

/// Production dependency bundle for one CLI invocation.
///
/// No ledger lock, synchronous mutex, or file lock is retained across any provider `.await`.
pub(crate) struct ProductionCompletionPorts<'a> {
    context: &'a ResolvedCommandContext<'a>,
    store: ConvergenceLedgerStore,
    range: String,
    command_authority: CommandAuthoritySnapshot,
    failure_ledger: CleanupFailureLedger,
    lease: Option<DetachedWorkspaceLease<CurrentCheckoutCleanup>>,
}

impl<'a> ProductionCompletionPorts<'a> {
    pub(crate) fn new(
        context: &'a ResolvedCommandContext<'a>,
        store: ConvergenceLedgerStore,
        range: &str,
        command_authority: CommandAuthoritySnapshot,
    ) -> Result<Self> {
        let ledger = store.load()?;
        let campaign_id = CampaignId::parse(
            context
                .args
                .campaign
                .as_deref()
                .context("validated completion campaign is missing")?,
        )?;
        let campaign = campaign_record(&ledger, &campaign_id)?;
        if campaign.command_authority() != &command_authority {
            bail!("current command authority differs from the clustered campaign authority");
        }
        Ok(Self {
            context,
            store,
            range: range.to_string(),
            command_authority,
            failure_ledger: CleanupFailureLedger::default(),
            lease: None,
        })
    }

    fn reserve_clean_room(
        &self,
        campaign_id: &CampaignId,
        epoch: &csa_session::convergence::EpochRecord,
        allowance: ProviderTurnAllowance,
    ) -> Result<ProviderTurnReservation> {
        if allowance.remaining_turns() == 0 {
            bail!("completion provider-turn budget is exhausted");
        }
        let policy_digest = campaign_record(&self.store.load()?, campaign_id)?
            .policy_digest()
            .cloned()
            .context("clustered campaign is missing its completion policy digest")?;
        let generation = match self.store.load_completion_action_journal()? {
            CompletionActionJournalRead::Missing => self
                .store
                .initialize_completion_action_journal(
                    campaign_id.clone(),
                    epoch.id().clone(),
                    policy_digest,
                )?
                .generation(),
            CompletionActionJournalRead::LegacyV1(_) => {
                bail!("legacy completion action journal cannot authorize clean-room execution")
            }
            CompletionActionJournalRead::Current(journal) => {
                if journal.campaign_id() != campaign_id
                    || journal.epoch_id() != epoch.id()
                    || !journal.permits_attestation()
                {
                    bail!("completion action journal does not permit this clean-room epoch");
                }
                journal.generation()
            }
        };
        let claim = self
            .store
            .claim_completion_action(generation, CompletionActionId::generate())?;
        self.store
            .reserve_completion_provider_turn(&claim, ProviderTurnExecutionId::generate(), 1)
            .map_err(Into::into)
    }

    fn acquire_lease(
        &mut self,
        campaign_id: &CampaignId,
        epoch: &csa_session::convergence::EpochRecord,
    ) -> Result<()> {
        if let Some(lease) = self.lease.as_ref() {
            if lease.identity().campaign_id() == campaign_id && lease.workspace().epoch() == epoch {
                lease.validate_current()?;
                return Ok(());
            }
            bail!("completion attempted to reuse a lease for a different campaign epoch");
        }
        let exact_evidence =
            bundle::build_exact_oid_evidence(self.context.project_root, &self.range)?;
        if exact_evidence.base_oid() != epoch.base_oid().as_str()
            || exact_evidence.head_oid() != epoch.head_oid().as_str()
            || exact_evidence.diff_digest() != epoch.diff_digest()
        {
            bail!("current checkout evidence differs from the clustered completion epoch");
        }
        let (_, published) = exact_evidence.publish(&self.store)?;
        let generation = self
            .store
            .load()?
            .generation()
            .checked_add(1)
            .context("lease generation overflow")?;
        let lease = acquire_current_checkout_lease(
            self.context.project_root,
            &published.path(),
            campaign_id.clone(),
            generation,
            epoch.clone(),
            COMPLETION_LEASE_TIMEOUT,
            self.failure_ledger.clone(),
        )?;
        let admitted = self
            .command_authority
            .ordered_admitted()
            .first()
            .cloned()
            .context("completion authority has no admitted executor")?;
        let authorization = CompletionAuthorizationEvent::new(
            campaign_id.clone(),
            epoch,
            0,
            admitted,
            &self.context.completion_policy,
            lease.identity().clone(),
        )?;
        self.store
            .append(campaign_id.clone(), authorization.ledger_event())?;
        self.lease = Some(lease);
        Ok(())
    }

    fn run_final_gates(
        &mut self,
        campaign_id: &CampaignId,
        epoch: &csa_session::convergence::EpochRecord,
    ) -> Result<ArtifactEvidenceRef> {
        self.acquire_lease(campaign_id, epoch)?;
        let lease = self
            .lease
            .as_ref()
            .context("completion lease disappeared before final gates")?;
        let authority = completion_gate_authority()?;
        let policy_digest = campaign_record(&self.store.load()?, campaign_id)?
            .policy_digest()
            .cloned()
            .context("clustered campaign is missing its completion policy digest")?;
        let plan = FinalGatePlan::from_authority(
            policy_digest,
            lease.identity().clone(),
            &authority,
            authority.commands().to_vec(),
        )?;
        let session = csa_session::create_session_fresh(
            self.context.project_root,
            Some("convergence final gates"),
            None,
            None,
        )?;
        let session_id = CsaSessionId::parse(&session.meta_session_id)?;
        let session_dir =
            csa_session::get_session_dir(self.context.project_root, session_id.as_str())?;
        let artifacts = HostGateArtifactStore::new(
            &session_dir.join("output"),
            session_id,
            SessionRelativeArtifactPath::new("output")?,
        )?;
        let mut port = HostFinalGatePort::new(BlockingDirectFinalGateDriver, artifacts);
        Ok(port.run(&plan, lease)?.artifact().clone())
    }

    async fn run_clean_room(
        &mut self,
        campaign_id: &CampaignId,
        epoch: &csa_session::convergence::EpochRecord,
        gate_artifact: &ArtifactEvidenceRef,
        reservation: &ProviderTurnReservation,
    ) -> Result<CompletedCleanRoom> {
        let lease = self
            .lease
            .as_ref()
            .context("clean-room execution requires the final-gate lease")?;
        lease.validate_current()?;
        let admitted = self.admitted_clean_room_executor().await?;
        let environment = ProviderEnvironmentInputs::new(
            allowed_provider_environment(&self.command_authority)?,
            BTreeMap::new(),
        );
        let provider_authority = ProviderCommandAuthority::capture(
            &admitted,
            &self.command_authority,
            environment,
            self.context.project_root,
            &SystemProviderProgramResolver,
        )?;
        let prompt = ExactProviderPrompt::new(build_clean_room_prompt(epoch, gate_artifact));
        let request = ProviderSessionRequest::from_authority(
            lease.workspace(),
            &self.command_authority,
            prompt,
        )?;
        let limits = crate::pipeline::CleanRoomExecutionLimits::try_new(
            self.context.idle_timeout_seconds,
            self.context.initial_response_timeout_seconds,
            self.context.args.timeout.map(Duration::from_secs),
            self.context.args.resource_overrides(),
            self.context.tier_name.clone(),
        )?;
        let mut provider = ProductionCleanRoomProvider::new(
            &admitted,
            &provider_authority,
            self.context.project_config,
            Some(self.context.global_config),
            limits,
        )?;
        let outcome = provider.run(&request).await?;
        let reconciliation =
            reconciliation_from_completion(reservation.clone(), outcome.provider_turn_completion());
        let observed_turn_delta = match &reconciliation {
            ProviderTurnReconciliation::Reconciled {
                host_observed_turn_delta,
                ..
            } => *host_observed_turn_delta,
            _ => unreachable!("successful provider execution always has a bounded reconciliation"),
        };
        self.store
            .reconcile_completion_provider_turn(reservation, observed_turn_delta)?;
        self.store.finish_completion_action(reservation.claim())?;
        let session_id = CsaSessionId::parse(outcome.session_id())?;
        let session_dir =
            csa_session::get_session_dir(self.context.project_root, session_id.as_str())?;
        let tool_version = crate::tool_version::detect_tool_version(&admitted)
            .await
            .unwrap_or_else(|| "host-version-unavailable".to_string());
        let model_evidence = ModelEvidence::host_observed(
            provider_authority.selected_identity().clone(),
            ObservedToolEvidence::new(provider_authority.runtime_binary(), &tool_version)?,
            None,
            reservation.execution_id().clone(),
        )?;
        let artifacts = HostReviewArtifactStore::new(
            &session_dir.join("output"),
            session_id,
            SessionRelativeArtifactPath::new("output")?,
        )?;
        let output = artifacts.publish(
            &ReviewEnvelopeContext::new(
                campaign_id.clone(),
                epoch.clone(),
                gate_artifact.clone(),
                model_evidence,
            ),
            std::str::from_utf8(outcome.artifact())
                .context("clean-room provider output was not UTF-8")?,
        )?;
        Ok(CompletedCleanRoom {
            output,
            reconciliation,
        })
    }

    async fn admitted_clean_room_executor(&self) -> Result<crate::pipeline::AdmittedExecutor> {
        let selected = self
            .command_authority
            .ordered_admitted()
            .first()
            .context("completion authority has no admitted executor")?;
        let model_spec = format!(
            "{}/{}/{}/{}",
            selected.tool(),
            selected.provider(),
            selected.model(),
            selected.reasoning()
        );
        let admitted = crate::pipeline::build_and_validate_executor(
            &self.context.tool,
            Some(&model_spec),
            None,
            None,
            crate::pipeline::ConfigRefs {
                project: self.context.project_config,
                global: Some(self.context.global_config),
                model_catalog: Some(self.context.model_catalog),
            },
            false,
            self.context.args.force_override_user_config,
            false,
        )
        .await?;
        if super::clean_room::admitted_identity(&admitted)? != *selected {
            bail!("fresh clean-room executor differs from the immutable command authority");
        }
        Ok(admitted)
    }

    fn publish_final_pair(
        &mut self,
        campaign_id: &CampaignId,
        epoch: &csa_session::convergence::EpochRecord,
        gate_artifact: &ArtifactEvidenceRef,
        clean_room: &super::clean_room_v2::CleanRoomReviewOutput,
    ) -> Result<()> {
        let lease = self
            .lease
            .take()
            .context("terminal publication requires the owned completion lease")?;
        if !self.failure_ledger.failures().is_empty() {
            bail!("completion lease has a recorded cleanup failure");
        }
        let cleanup_confirmation = lease.close_and_confirm()?;
        if !self.failure_ledger.failures().is_empty() {
            bail!("completion lease cleanup fallback reported a failure");
        }
        let authority = completion_gate_authority()?;
        let policy_digest = campaign_record(&self.store.load()?, campaign_id)?
            .policy_digest()
            .cloned()
            .context("clustered campaign is missing its completion policy digest")?;
        let commands = authority
            .commands()
            .iter()
            .map(|command| GateCommandResult::new(command.command_id(), 0))
            .collect::<Result<Vec<_>>>()?;
        let gate = GateEvidenceRecord::new(
            campaign_id.clone(),
            epoch,
            policy_digest,
            authority.digest(),
            commands,
            gate_artifact.clone(),
        )?;
        let final_review = CleanRoomReviewRecord::new(
            campaign_id.clone(),
            epoch,
            clean_room.model_evidence().clone(),
            CleanRoomReviewArtifactBindings::new(
                gate_artifact.clone(),
                clean_room.artifact().clone(),
            ),
            u32::try_from(clean_room.findings().len())?,
            u32::try_from(clean_room.questions().len())?,
            u32::try_from(clean_room.unchecked_items().len())?,
        )?;
        let reader = |artifact: &ArtifactEvidenceRef| {
            let file_name = artifact
                .path()
                .as_str()
                .strip_prefix("output/")
                .context("terminal artifact is outside a session output directory")?;
            csa_session::read_session_output_artifact(
                &csa_session::get_session_dir(
                    self.context.project_root,
                    artifact.csa_session_id().as_str(),
                )?,
                file_name,
            )
        };
        self.store.publish_verified_final_attestation(
            campaign_id.clone(),
            gate,
            final_review,
            cleanup_confirmation,
            &reader,
        )?;
        Ok(())
    }
}

impl CompletionPorts for ProductionCompletionPorts<'_> {
    fn reserve_execution<'a>(
        &'a mut self,
        action: &'a CompletionAction,
        allowance: ProviderTurnAllowance,
    ) -> Pin<
        Box<dyn Future<Output = Result<CompletionExecutionReservation, CompletionPortError>> + 'a>,
    > {
        Box::pin(async move {
            match action {
                CompletionAction::RunFreshCleanRoom {
                    campaign_id, epoch, ..
                } => self
                    .reserve_clean_room(campaign_id, epoch, allowance)
                    .map(CompletionExecutionReservation::Provider)
                    .map_err(port_error),
                _ => Ok(CompletionExecutionReservation::HostOnly),
            }
        })
    }

    fn execute<'a>(
        &'a mut self,
        action: &'a CompletionAction,
        reservation: &'a CompletionExecutionReservation,
    ) -> Pin<Box<dyn Future<Output = CompletionPortResult> + 'a>> {
        Box::pin(async move {
            let host = || ProviderTurnReconciliation::HostOnly;
            match action {
                CompletionAction::RunFinalGates { campaign_id, epoch } => CompletionPortResult {
                    event: self
                        .run_final_gates(campaign_id, epoch)
                        .map(|artifact| CompletionEvent::FinalGatesPassed {
                            campaign_id: campaign_id.clone(),
                            epoch_id: epoch.id().clone(),
                            artifact,
                        })
                        .map_err(port_error),
                    reconciliation: host(),
                },
                CompletionAction::RunFreshCleanRoom {
                    campaign_id,
                    epoch,
                    gate_artifact,
                } => {
                    let CompletionExecutionReservation::Provider(provider_reservation) =
                        reservation
                    else {
                        return CompletionPortResult {
                            event: Err(CompletionPortError::new(
                                "clean-room execution lacks a provider reservation",
                            )),
                            reconciliation: ProviderTurnReconciliation::UsageIndeterminate {
                                reservation: None,
                            },
                        };
                    };
                    match self
                        .run_clean_room(campaign_id, epoch, gate_artifact, provider_reservation)
                        .await
                    {
                        Ok(completed) => CompletionPortResult {
                            event: Ok(CompletionEvent::CleanRoomCompleted {
                                campaign_id: campaign_id.clone(),
                                epoch_id: epoch.id().clone(),
                                output: completed.output,
                            }),
                            reconciliation: completed.reconciliation,
                        },
                        Err(error) => {
                            let _ = self
                                .store
                                .mark_completion_provider_turn_usage_indeterminate(
                                    provider_reservation,
                                );
                            CompletionPortResult {
                                event: Err(port_error(error)),
                                reconciliation: ProviderTurnReconciliation::UsageIndeterminate {
                                    reservation: Some(provider_reservation.clone()),
                                },
                            }
                        }
                    }
                }
                CompletionAction::PublishFinalPair {
                    campaign_id,
                    epoch,
                    gate_artifact,
                    clean_room,
                } => CompletionPortResult {
                    event: self
                        .publish_final_pair(campaign_id, epoch, gate_artifact, clean_room)
                        .map(|()| CompletionEvent::FinalPairPublished {
                            campaign_id: campaign_id.clone(),
                            epoch_id: epoch.id().clone(),
                            gate_artifact: gate_artifact.clone(),
                            review_artifact: clean_room.artifact().clone(),
                            model_evidence: clean_room.model_evidence().clone(),
                        })
                        .map_err(port_error),
                    reconciliation: host(),
                },
                CompletionAction::RunAuthorizedRepairs { .. }
                | CompletionAction::Discover { .. }
                | CompletionAction::VerifyAndCluster { .. } => CompletionPortResult {
                    event: Err(CompletionPortError::new(
                        "this clustered completion requires a repair or rediscovery port; restart through the explicit repair workflow",
                    )),
                    reconciliation: host(),
                },
            }
        })
    }
}

struct CompletedCleanRoom {
    output: super::clean_room_v2::CleanRoomReviewOutput,
    reconciliation: ProviderTurnReconciliation,
}

fn completion_gate_authority() -> Result<FinalGateAuthority> {
    FinalGateAuthority::new(
        COMPLETION_GATE_AUTHORITY_VERSION,
        vec![
            network_denied_cargo_gate("fmt", ["fmt", "--all", "--", "--check"])?,
            network_denied_cargo_gate(
                "clippy",
                [
                    "clippy",
                    "--workspace",
                    "--all-features",
                    "--",
                    "-D",
                    "warnings",
                ],
            )?,
            network_denied_cargo_gate("test", ["test", "--workspace"])?,
        ],
    )
}

/// Keep final gates offline even when Cargo itself would otherwise attempt registry access.
///
/// `unshare --net -- cargo …` is direct argv, not a shell. If this Linux isolation primitive is
/// unavailable, the command fails closed and no gate artifact is published.
fn network_denied_cargo_gate<const N: usize>(
    command_id: &str,
    cargo_args: [&str; N],
) -> Result<GateCommandAuthority> {
    let mut argv = vec!["--net".to_string(), "--".to_string(), "cargo".to_string()];
    argv.extend(cargo_args.into_iter().map(str::to_string));
    GateCommandAuthority::new(
        command_id,
        "unshare",
        argv,
        GateNetworkPolicy::Denied,
        COMPLETION_GATE_TIMEOUT,
    )
}

fn port_error(error: impl std::fmt::Display) -> CompletionPortError {
    CompletionPortError::new(error.to_string())
}

pub(crate) const fn completion_budget() -> (u32, u32) {
    (COMPLETION_MAX_CYCLES, COMPLETION_MAX_PROVIDER_TURNS)
}
