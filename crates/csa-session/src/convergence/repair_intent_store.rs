//! Secure, durable storage for repair intents owned by completion action claims.

use std::ffi::OsStr;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, anyhow, bail};
use fd_lock::RwLock;
use thiserror::Error;

use super::secure_fs::{self, SecureDirectory};
use super::{
    CompletionActionClaim, CompletionActionJournalRead, CompletionActionState,
    ConvergenceLedgerStore, EpochRecord, RepairIntent,
};
use crate::atomic_state_write::{self, AtomicPublishError};

const MAX_REPAIR_INTENT_BYTES: u64 = 1024 * 1024;

/// Result of inspecting a claim-addressed repair intent without mutating it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairIntentRead {
    /// No durable intent exists for the requested action claim.
    Missing,
    /// A validated repair intent exists for the requested action claim.
    Current(Box<RepairIntent>),
}

/// Failure while publishing or reconciling a repair intent.
#[derive(Debug, Error)]
pub enum RepairIntentStoreError {
    /// The requested transition was not published, so repair execution must not begin.
    #[error("repair intent was not published: {0:#}")]
    NotPublished(#[source] anyhow::Error),
    /// The rename may have happened; reload and fail closed before any recovery decision.
    #[error(
        "repair intent may have been published, but durability is unconfirmed; reload before recovery: {0:#}"
    )]
    PublishedButDurabilityUnconfirmed(#[source] anyhow::Error),
}

impl RepairIntentStoreError {
    /// Whether the requested transition may already be visible on disk.
    #[must_use]
    pub fn may_have_been_published(&self) -> bool {
        matches!(self, Self::PublishedButDurabilityUnconfirmed(_))
    }
}

impl ConvergenceLedgerStore {
    /// Read one repair intent without creating, resuming, or repairing it.
    pub fn load_repair_intent(
        &self,
        claim: &CompletionActionClaim,
    ) -> anyhow::Result<RepairIntentRead> {
        let Some(directory) = secure_fs::open_convergence_directory(
            self.secure_boundary(),
            self.project_state_root(),
            false,
        )?
        else {
            return Ok(RepairIntentRead::Missing);
        };
        directory.verify_link()?;
        let read = self.load_repair_intent_from_directory(&directory, claim)?;
        directory.verify_link()?;
        Ok(read)
    }

    /// Persist a started intent before the claim holder may mutate the source repository.
    ///
    /// The claim is checked under the same project lock as the intent write, so a stale holder
    /// cannot create a new repair authority after recovery has fenced it out.
    pub fn persist_repair_intent(
        &self,
        intent: RepairIntent,
    ) -> Result<(), RepairIntentStoreError> {
        let claim = intent.claim().clone();
        self.with_repair_intent_lock(&claim, |directory| {
            intent.validate().map_err(intent_not_published)?;
            match self
                .load_repair_intent_from_directory(directory, &claim)
                .map_err(intent_not_published)?
            {
                RepairIntentRead::Missing => self.publish_repair_intent(directory, &intent),
                RepairIntentRead::Current(_) => Err(intent_not_published(anyhow!(
                    "repair intent already exists for completion action {}",
                    claim.action_id()
                ))),
            }
        })
    }

    /// Mark a started intent committed only after the caller has independently verified the
    /// matching changed source epoch and corresponding ledger epoch transaction.
    pub fn mark_repair_intent_committed(
        &self,
        claim: &CompletionActionClaim,
        observed_epoch: EpochRecord,
    ) -> Result<(), RepairIntentStoreError> {
        self.update_repair_intent(claim, |intent| intent.mark_committed(observed_epoch))
    }

    /// Mark recovery uncertain without guessing repair success or rolling source state back.
    pub fn mark_repair_intent_uncertain(
        &self,
        claim: &CompletionActionClaim,
    ) -> Result<(), RepairIntentStoreError> {
        self.update_repair_intent(claim, RepairIntent::mark_uncertain)
    }

    fn update_repair_intent(
        &self,
        claim: &CompletionActionClaim,
        update: impl FnOnce(&mut RepairIntent) -> anyhow::Result<()>,
    ) -> Result<(), RepairIntentStoreError> {
        self.with_repair_intent_lock(claim, |directory| {
            let RepairIntentRead::Current(mut intent) = self
                .load_repair_intent_from_directory(directory, claim)
                .map_err(intent_not_published)?
            else {
                return Err(intent_not_published(anyhow!(
                    "repair intent is missing for completion action {}",
                    claim.action_id()
                )));
            };
            update(&mut intent).map_err(intent_not_published)?;
            self.publish_repair_intent(directory, &intent)
        })
    }

    fn with_repair_intent_lock(
        &self,
        claim: &CompletionActionClaim,
        operation: impl FnOnce(&SecureDirectory) -> Result<(), RepairIntentStoreError>,
    ) -> Result<(), RepairIntentStoreError> {
        let directory = secure_fs::open_convergence_directory(
            self.secure_boundary(),
            self.project_state_root(),
            true,
        )
        .map_err(intent_not_published)?
        .ok_or_else(|| {
            intent_not_published(anyhow!("secure convergence directory was not opened"))
        })?;
        let lock_file = directory
            .open_lock(super::store::lock_name())
            .context("securely open the convergence repair-intent lock")
            .map_err(intent_not_published)?;
        let mut lock = RwLock::new(lock_file);
        let _guard = lock.write().map_err(|error| {
            intent_not_published(anyhow!(error).context("acquire repair-intent lock"))
        })?;
        directory.verify_link().map_err(intent_not_published)?;
        self.require_current_started_action_claim(&directory, claim)
            .map_err(intent_not_published)?;
        operation(&directory)?;
        directory.verify_link().map_err(intent_uncertain)
    }

    fn require_current_started_action_claim(
        &self,
        directory: &SecureDirectory,
        claim: &CompletionActionClaim,
    ) -> anyhow::Result<()> {
        let journal = match self.load_completion_action_journal_from_directory(directory)? {
            CompletionActionJournalRead::Current(journal) => journal,
            CompletionActionJournalRead::Missing => bail!("completion action journal is missing"),
            CompletionActionJournalRead::LegacyV1(_) => {
                bail!("legacy completion action journal cannot authorize repair intent")
            }
        };
        if journal.generation() != claim.generation()
            || journal.campaign_id() != claim.campaign_id()
            || journal.epoch_id() != claim.epoch_id()
            || journal.policy_digest() != claim.policy_digest()
        {
            bail!("repair intent completion action claim is stale or mismatched");
        }
        let current = journal
            .actions()
            .last()
            .context("completion action journal has no current action")?;
        if current.claim() != claim || current.state() != CompletionActionState::Started {
            bail!("repair intent requires the current started completion action claim");
        }
        Ok(())
    }

    fn load_repair_intent_from_directory(
        &self,
        directory: &SecureDirectory,
        claim: &CompletionActionClaim,
    ) -> anyhow::Result<RepairIntentRead> {
        let name = repair_intent_name(claim);
        let path = repair_intent_path(self, &name);
        let Some(file) = directory
            .open_private_file(OsStr::new(&name))
            .with_context(|| format!("securely open repair intent {}", path.display()))?
        else {
            return Ok(RepairIntentRead::Missing);
        };
        let intent = read_repair_intent(file, &path)?;
        if intent.claim() != claim {
            bail!(
                "repair intent {} does not match the requested completion action claim",
                path.display()
            );
        }
        Ok(RepairIntentRead::Current(Box::new(intent)))
    }

    fn publish_repair_intent(
        &self,
        directory: &SecureDirectory,
        intent: &RepairIntent,
    ) -> Result<(), RepairIntentStoreError> {
        let name = repair_intent_name(intent.claim());
        let path = repair_intent_path(self, &name);
        let bytes = serialize_repair_intent(intent).map_err(intent_not_published)?;
        atomic_state_write::publish_bytes_in(
            directory.file(),
            Some(directory.parent()),
            OsStr::new(&name),
            &path,
            &bytes,
        )
        .map_err(map_intent_publish_error)
    }
}

fn repair_intent_name(claim: &CompletionActionClaim) -> String {
    format!("repair-intent-{}.json", claim.action_id().as_str())
}

fn repair_intent_path(store: &ConvergenceLedgerStore, name: &str) -> std::path::PathBuf {
    store.project_state_root().join("convergence").join(name)
}

fn read_repair_intent(file: File, path: &Path) -> anyhow::Result<RepairIntent> {
    let metadata = file
        .metadata()
        .with_context(|| format!("inspect repair intent {}", path.display()))?;
    if metadata.len() > MAX_REPAIR_INTENT_BYTES {
        bail!(
            "repair intent exceeds maximum size of {MAX_REPAIR_INTENT_BYTES} bytes: {}",
            path.display()
        );
    }
    let mut bytes = Vec::new();
    file.take(MAX_REPAIR_INTENT_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read repair intent {}", path.display()))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_REPAIR_INTENT_BYTES {
        bail!(
            "repair intent grew beyond maximum size of {MAX_REPAIR_INTENT_BYTES} bytes: {}",
            path.display()
        );
    }
    let intent: RepairIntent = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse repair intent {}", path.display()))?;
    intent.validate()?;
    Ok(intent)
}

fn serialize_repair_intent(intent: &RepairIntent) -> anyhow::Result<Vec<u8>> {
    intent.validate()?;
    let mut bytes = serde_json::to_vec_pretty(intent).context("serialize repair intent")?;
    bytes.push(b'\n');
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_REPAIR_INTENT_BYTES {
        bail!("serialized repair intent exceeds maximum size of {MAX_REPAIR_INTENT_BYTES} bytes");
    }
    let roundtrip: RepairIntent =
        serde_json::from_slice(&bytes).context("round-trip repair intent serialization")?;
    if &roundtrip != intent {
        bail!("serialized repair intent did not round-trip exactly");
    }
    Ok(bytes)
}

fn intent_not_published(error: anyhow::Error) -> RepairIntentStoreError {
    RepairIntentStoreError::NotPublished(error)
}

fn intent_uncertain(error: anyhow::Error) -> RepairIntentStoreError {
    RepairIntentStoreError::PublishedButDurabilityUnconfirmed(error)
}

fn map_intent_publish_error(error: AtomicPublishError) -> RepairIntentStoreError {
    match error {
        AtomicPublishError::BeforePublish(error) => intent_not_published(error),
        AtomicPublishError::PublishedButDurabilityUnconfirmed(error) => intent_uncertain(error),
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::convergence::{
        CampaignId, CompletionActionId, GitObjectId, RepairBatchId, RepairIntentState, Sha256Digest,
    };

    fn epoch(head: char) -> EpochRecord {
        EpochRecord::new(
            GitObjectId::parse("1111111111111111111111111111111111111111").unwrap(),
            GitObjectId::parse(&head.to_string().repeat(40)).unwrap(),
            Sha256Digest::compute(b"diff"),
        )
    }

    fn initialized_store() -> (
        TempDir,
        ConvergenceLedgerStore,
        CompletionActionClaim,
        EpochRecord,
    ) {
        let temp = TempDir::new().unwrap();
        let store = ConvergenceLedgerStore::for_project_state_root(temp.path()).unwrap();
        let expected = epoch('2');
        let campaign = CampaignId::generate();
        let policy = Sha256Digest::compute(b"policy");
        store
            .initialize_completion_action_journal(campaign, expected.id().clone(), policy)
            .unwrap();
        let claim = store
            .claim_completion_action(0, CompletionActionId::generate())
            .unwrap();
        (temp, store, claim, expected)
    }

    fn intent(claim: CompletionActionClaim, expected: EpochRecord) -> RepairIntent {
        RepairIntent::new(
            claim,
            expected,
            Sha256Digest::compute(b"batches"),
            vec![RepairBatchId::generate()],
        )
        .unwrap()
    }

    #[test]
    fn store_persists_claim_bound_intent_and_changed_epoch_reconciliation() {
        let (_temp, store, claim, expected) = initialized_store();
        store
            .persist_repair_intent(intent(claim.clone(), expected))
            .unwrap();
        assert!(matches!(
            store.load_repair_intent(&claim).unwrap(),
            RepairIntentRead::Current(value) if matches!(value.state(), RepairIntentState::Started)
        ));

        store
            .mark_repair_intent_committed(&claim, epoch('3'))
            .unwrap();
        assert!(matches!(
            store.load_repair_intent(&claim).unwrap(),
            RepairIntentRead::Current(value)
                if matches!(value.state(), RepairIntentState::Committed { .. })
        ));
    }

    #[test]
    fn stale_action_claim_cannot_persist_or_change_repair_intent() {
        let (_temp, store, claim, expected) = initialized_store();
        let stale = claim.clone();
        store
            .recover_completion_action(&claim, CompletionActionId::generate())
            .unwrap();

        let error = store
            .persist_repair_intent(intent(stale, expected))
            .unwrap_err();
        assert!(error.to_string().contains("stale or mismatched"));
    }
}
