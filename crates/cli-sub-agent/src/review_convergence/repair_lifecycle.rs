//! Fenced repair-intent lifecycle and deterministic crash reconciliation.

use std::path::Path;

use anyhow::{Context, Result, bail};
use csa_session::convergence::{
    CampaignId, CompletionActionClaim, CompletionActionId, CompletionActionJournalRead,
    CompletionActionState, ConsolidatedRepairAuthorization, ConvergenceEvent, ConvergenceLedger,
    ConvergenceLedgerStore, EpochRecord, RepairBatchRecord, RepairIntent, RepairIntentRead,
    RepairIntentState,
};

use super::repair_source::capture_epoch;

/// Recover a started intent only when source and ledger independently prove the exact commit.
pub(super) fn reconcile_incomplete_repair_intent(
    store: &ConvergenceLedgerStore,
    project_root: &Path,
    campaign_id: &CampaignId,
) -> Result<()> {
    let CompletionActionJournalRead::Current(journal) = store.load_completion_action_journal()?
    else {
        return Ok(());
    };
    if journal.campaign_id() != campaign_id {
        bail!(
            "completion action journal belongs to campaign {}; refusing cross-campaign repair resume",
            journal.campaign_id()
        );
    }
    let Some(action) = journal.actions().last() else {
        return Ok(());
    };
    match action.state() {
        CompletionActionState::Finished => Ok(()),
        CompletionActionState::Uncertain => bail!(
            "completion action {} is uncertain; repair cannot resume",
            action.claim().action_id()
        ),
        CompletionActionState::Started => {
            let claim = action.claim();
            let intent = match store.load_repair_intent(claim) {
                Ok(RepairIntentRead::Current(intent)) => intent,
                Ok(RepairIntentRead::Missing) => {
                    store
                        .mark_completion_action_uncertain(claim)
                        .map_err(anyhow::Error::from)?;
                    bail!(
                        "started repair action {} has no durable repair intent",
                        claim.action_id()
                    );
                }
                Err(error) => {
                    store
                        .mark_completion_action_uncertain(claim)
                        .map_err(anyhow::Error::from)?;
                    return Err(error).context("started repair intent could not be read");
                }
            };
            let observed = capture_epoch(project_root, intent.expected_epoch().base_oid());
            let ledger = store.load();
            let committed = match (&observed, &ledger) {
                (Ok(observed), Ok(ledger)) if observed.clean => {
                    repair_intent_matches_committed_epoch(&intent, &observed.epoch, ledger)
                }
                _ => false,
            };
            if committed {
                if matches!(intent.state(), RepairIntentState::Started) {
                    store
                        .mark_repair_intent_committed(claim, observed?.epoch)
                        .map_err(anyhow::Error::from)?;
                }
                store
                    .finish_completion_action(claim)
                    .map_err(anyhow::Error::from)?;
                bail!(
                    "repair action {} was recovered from its committed epoch; restart from the new epoch",
                    claim.action_id()
                );
            }
            mark_repair_uncertain(store, claim)?;
            bail!(
                "repair action {} has an incomplete or drifting source/ledger combination",
                claim.action_id()
            );
        }
    }
}

/// Claim the next action only for the exact campaign, epoch, and policy of an authorization.
pub(super) fn claim_repair_action(
    store: &ConvergenceLedgerStore,
    authorization: &ConsolidatedRepairAuthorization,
) -> Result<CompletionActionClaim> {
    let policy_digest = authorization
        .campaign()
        .policy_digest()
        .context("repair campaign is missing its immutable completion policy digest")?
        .clone();
    let generation = match store.load_completion_action_journal_for(
        authorization.campaign().id(),
        authorization.epoch().id(),
        &policy_digest,
    )? {
        CompletionActionJournalRead::Missing => store
            .initialize_completion_action_journal(
                authorization.campaign().id().clone(),
                authorization.epoch().id().clone(),
                policy_digest.clone(),
            )
            .map_err(anyhow::Error::from)?
            .generation(),
        CompletionActionJournalRead::LegacyV1(_) => {
            bail!("legacy completion action journal cannot authorize repair")
        }
        CompletionActionJournalRead::Current(journal) => {
            if journal.campaign_id() != authorization.campaign().id()
                || journal.epoch_id() != authorization.epoch().id()
                || journal.policy_digest() != &policy_digest
            {
                bail!("completion action journal does not match the authorized repair epoch");
            }
            if !journal.permits_attestation() {
                bail!("completion action journal contains an unresolved action");
            }
            journal.generation()
        }
    };
    store
        .claim_completion_action(generation, CompletionActionId::generate())
        .map_err(anyhow::Error::from)
}

/// Construct the only exact repair selection that a writer is permitted to execute.
pub(super) fn repair_intent(
    authorization: &ConsolidatedRepairAuthorization,
    claim: CompletionActionClaim,
) -> Result<RepairIntent> {
    let batch_ids = authorization
        .batches()
        .iter()
        .map(|batch| batch.id().clone())
        .collect();
    RepairIntent::new(
        claim,
        authorization.epoch().clone(),
        RepairBatchRecord::set_digest(authorization.batches()),
        batch_ids,
    )
}

/// Reject every repair selection that is not an exact ledger-authorized batch set.
pub(super) fn validate_exact_repair_batches(
    authorization: &ConsolidatedRepairAuthorization,
    intent: &RepairIntent,
) -> Result<()> {
    if intent.claim().campaign_id() != authorization.campaign().id()
        || intent.expected_epoch() != authorization.epoch()
        || intent.repair_batch_set_digest()
            != &RepairBatchRecord::set_digest(authorization.batches())
    {
        bail!("repair intent does not match the ledger-authorized repair authority");
    }
    let mut authorized = authorization
        .batches()
        .iter()
        .map(|batch| batch.id().clone())
        .collect::<Vec<_>>();
    authorized.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    if authorized != intent.repair_batch_ids() {
        bail!("repair intent does not contain the exact ledger-authorized repair batch IDs");
    }
    Ok(())
}

fn repair_intent_matches_committed_epoch(
    intent: &RepairIntent,
    observed_epoch: &EpochRecord,
    ledger: &ConvergenceLedger,
) -> bool {
    if ledger.validate().is_err()
        || observed_epoch.base_oid() != intent.expected_epoch().base_oid()
        || observed_epoch.head_oid() == intent.expected_commit()
    {
        return false;
    }
    let entries = ledger
        .entries()
        .iter()
        .filter(|entry| entry.campaign_id() == intent.claim().campaign_id())
        .collect::<Vec<_>>();
    let Some(expected_epoch_position) = entries.iter().rposition(|entry| {
        matches!(entry.event(), ConvergenceEvent::EpochOpened(epoch) if epoch == intent.expected_epoch())
    }) else {
        return false;
    };
    let Some((last, epoch_entries)) = entries.split_last() else {
        return false;
    };
    if !matches!(last.event(), ConvergenceEvent::EpochOpened(epoch) if epoch == observed_epoch) {
        return false;
    }
    let mut authorized_batches = Vec::new();
    let mut completed = Vec::new();
    for entry in &epoch_entries[expected_epoch_position + 1..] {
        match entry.event() {
            ConvergenceEvent::EpochOpened(_) => return false,
            ConvergenceEvent::RepairBatchRecorded(record)
                if record.epoch_id() == intent.expected_epoch().id() =>
            {
                authorized_batches.push(record.clone());
            }
            ConvergenceEvent::RepairHandoffRecorded(record) => {
                if record.epoch_id() != intent.expected_epoch().id()
                    || record.repair_batch_set_digest() != intent.repair_batch_set_digest()
                {
                    return false;
                }
                completed.push(record.repair_batch_id().clone());
            }
            _ => {}
        }
    }
    if RepairBatchRecord::set_digest(&authorized_batches) != *intent.repair_batch_set_digest() {
        return false;
    }
    let mut authorized = authorized_batches
        .iter()
        .map(|batch| batch.id().clone())
        .collect::<Vec<_>>();
    authorized.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    if authorized != intent.repair_batch_ids() {
        return false;
    }
    completed.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    completed == intent.repair_batch_ids()
}

/// Preserve the original failure while making the repair action terminally non-attestable.
pub(super) fn finish_failed_repair(
    store: &ConvergenceLedgerStore,
    claim: &CompletionActionClaim,
    error: anyhow::Error,
) -> Result<i32> {
    match mark_repair_uncertain(store, claim) {
        Ok(()) => Err(error),
        Err(recovery_error) => Err(error.context(format!(
            "repair recovery could not be marked uncertain: {recovery_error:#}"
        ))),
    }
}

fn mark_repair_uncertain(
    store: &ConvergenceLedgerStore,
    claim: &CompletionActionClaim,
) -> Result<()> {
    let intent_result = match store.load_repair_intent(claim) {
        Ok(RepairIntentRead::Current(intent))
            if matches!(intent.state(), RepairIntentState::Started) =>
        {
            store
                .mark_repair_intent_uncertain(claim)
                .map_err(anyhow::Error::from)
        }
        Ok(RepairIntentRead::Current(_)) | Ok(RepairIntentRead::Missing) => Ok(()),
        Err(error) => Err(error),
    };
    let action_result = store
        .mark_completion_action_uncertain(claim)
        .map_err(anyhow::Error::from);
    match (intent_result, action_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(intent_error), Ok(())) => Err(intent_error),
        (Ok(()), Err(action_error)) => Err(action_error),
        (Err(intent_error), Err(action_error)) => Err(intent_error.context(format!(
            "completion action could not be marked uncertain: {action_error:#}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use csa_session::convergence::{
        AdmittedModelIdentity, ArtifactEvidenceRef, CompletionActionJournalRead,
        CompletionActionState, ConvergenceEvent, CsaSessionId, EpochRecord, GitObjectId,
        RepairBatchId, RepairBatchRecord, RepairHandoffRecord, RootClusterRecord,
        SessionRelativeArtifactPath, Sha256Digest,
    };
    use tempfile::TempDir;

    use super::{
        CampaignId, CompletionActionId, ConvergenceLedgerStore, RepairIntent, RepairIntentRead,
        RepairIntentState, capture_epoch, reconcile_incomplete_repair_intent,
        repair_intent_matches_committed_epoch,
    };

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .expect("run fixture git command");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repository() -> TempDir {
        let temp = TempDir::new().expect("temporary repository");
        git(temp.path(), &["init"]);
        git(
            temp.path(),
            &["config", "user.name", "repair lifecycle test"],
        );
        git(
            temp.path(),
            &["config", "user.email", "repair-lifecycle@example.invalid"],
        );
        fs::write(temp.path().join("repair.txt"), "initial\n").expect("fixture source");
        git(temp.path(), &["add", "repair.txt"]);
        git(temp.path(), &["commit", "-m", "initial"]);
        temp
    }

    /// Isolate journal/state writes from the host `XDG_STATE_HOME` before
    /// `ConvergenceLedgerStore::for_project` resolves secure state paths.
    fn isolated_project_store(
        repository: &Path,
    ) -> (
        crate::test_env_lock::ScopedTestEnvVar,
        ConvergenceLedgerStore,
    ) {
        let state_home = repository.join("xdg-state");
        fs::create_dir_all(&state_home).expect("create isolated XDG_STATE_HOME");
        let state_guard =
            crate::test_env_lock::ScopedTestEnvVar::set("XDG_STATE_HOME", &state_home);
        let store = ConvergenceLedgerStore::for_project(repository)
            .expect("open project convergence store under isolated XDG_STATE_HOME");
        (state_guard, store)
    }

    fn head(root: &Path) -> csa_session::convergence::GitObjectId {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("read fixture HEAD");
        assert!(output.status.success(), "fixture HEAD must exist");
        csa_session::convergence::GitObjectId::parse(
            String::from_utf8(output.stdout)
                .expect("UTF-8 fixture HEAD")
                .trim(),
        )
        .expect("valid fixture HEAD")
    }

    #[test]
    fn crash_before_ledger_commit_marks_repo_ledger_combination_uncertain() {
        let repository = repository();
        let expected = capture_epoch(repository.path(), &head(repository.path()))
            .expect("capture immutable expected epoch")
            .epoch;
        let (_state_home, store) = isolated_project_store(repository.path());
        let campaign = CampaignId::generate();
        let policy = Sha256Digest::compute(b"repair lifecycle policy");
        store
            .initialize_completion_action_journal(campaign.clone(), expected.id().clone(), policy)
            .expect("initialize action journal");
        let claim = store
            .claim_completion_action(0, CompletionActionId::generate())
            .expect("claim repair action");
        let intent = RepairIntent::new(
            claim.clone(),
            expected,
            Sha256Digest::compute(b"exact repair batches"),
            vec![RepairBatchId::generate()],
        )
        .expect("create durable repair intent");
        store
            .persist_repair_intent(intent)
            .expect("persist repair intent before mutation");

        let error = reconcile_incomplete_repair_intent(&store, repository.path(), &campaign)
            .expect_err("unchanged source with no committed epoch must not be guessed successful");
        assert!(
            error
                .to_string()
                .contains("incomplete or drifting source/ledger combination"),
            "unexpected reconciliation error: {error:#}"
        );
        assert!(matches!(
            store.load_repair_intent(&claim).expect("read repaired intent"),
            RepairIntentRead::Current(intent) if matches!(intent.state(), RepairIntentState::Uncertain)
        ));
        assert!(matches!(
            store
                .load_completion_action_journal()
                .expect("read action journal"),
            CompletionActionJournalRead::Current(journal)
                if matches!(journal.actions().last().map(|action| action.state()), Some(CompletionActionState::Uncertain))
        ));
        assert!(
            store.verify_completion_action_journal_attestable().is_err(),
            "uncertain repair state must block final attestation"
        );
    }

    #[test]
    fn recovery_accepts_only_the_exact_complete_handoff_and_changed_epoch_suffix() {
        let (mut ledger, clustered) = super::super::completion_resume_tests::clustered_claim(true);
        let campaign = ledger
            .entries()
            .iter()
            .find_map(|entry| match entry.event() {
                ConvergenceEvent::CampaignStarted(campaign) => Some(campaign.clone()),
                _ => None,
            })
            .expect("clustered fixture campaign");
        let batches = ledger
            .entries()
            .iter()
            .filter_map(|entry| match entry.event() {
                ConvergenceEvent::RepairBatchRecorded(batch) => Some(batch.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let roots = ledger
            .entries()
            .iter()
            .filter_map(|entry| match entry.event() {
                ConvergenceEvent::RootClusterRecorded(root) => Some(root.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let repository = repository();
        let (_state_home, store) = isolated_project_store(repository.path());
        store
            .initialize_completion_action_journal(
                campaign.id().clone(),
                clustered.epoch.id().clone(),
                clustered.policy_digest.clone(),
            )
            .expect("initialize action journal");
        let claim = store
            .claim_completion_action(0, CompletionActionId::generate())
            .expect("claim repair action");
        let intent = RepairIntent::new(
            claim,
            clustered.epoch.clone(),
            RepairBatchRecord::set_digest(&batches),
            batches.iter().map(|batch| batch.id().clone()).collect(),
        )
        .expect("bind exact authorized repair intent");
        let observed = EpochRecord::new(
            clustered.epoch.base_oid().clone(),
            GitObjectId::parse(&"33".repeat(20)).expect("changed fixture HEAD"),
            Sha256Digest::compute(b"changed fixture diff"),
        );
        assert!(
            !repair_intent_matches_committed_epoch(&intent, &observed, &ledger),
            "a crash before ledger publication must not be treated as completion"
        );

        let executor: AdmittedModelIdentity =
            campaign.command_authority().ordered_admitted()[0].clone();
        let artifact = ArtifactEvidenceRef::new(
            CsaSessionId::generate(),
            SessionRelativeArtifactPath::new("output/repair-recovery.json")
                .expect("fixture artifact path"),
            Sha256Digest::compute(b"repair recovery artifact"),
        );
        let cluster_set_digest = RootClusterRecord::set_digest(&roots);
        let repair_batch_set_digest = RepairBatchRecord::set_digest(&batches);
        let mut events = batches
            .iter()
            .map(|batch| {
                ConvergenceEvent::RepairHandoffRecorded(RepairHandoffRecord::new(
                    campaign.id().clone(),
                    clustered.epoch.id().clone(),
                    batch.id().clone(),
                    batch.content_digest().clone(),
                    campaign.command_authority_digest().clone(),
                    batch.candidate_set_digest().clone(),
                    batch.disposition_set_digest().clone(),
                    cluster_set_digest.clone(),
                    repair_batch_set_digest.clone(),
                    executor.clone(),
                    artifact.clone(),
                ))
            })
            .collect::<Vec<_>>();
        events.push(ConvergenceEvent::EpochOpened(observed.clone()));
        ledger
            .append_batch(campaign.id().clone(), events)
            .expect("append complete repair suffix");

        assert!(
            repair_intent_matches_committed_epoch(&intent, &observed, &ledger),
            "recovery requires the exact handoff set followed by the observed changed epoch"
        );
    }
}
