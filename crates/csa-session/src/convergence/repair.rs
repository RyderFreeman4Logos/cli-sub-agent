use std::{fmt, str::FromStr};

use anyhow::{Result, bail};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use ulid::Ulid;

use super::{
    ArtifactEvidenceRef, CampaignId, CandidateId, EpochId, Sha256Digest, hash_fields,
    normalize_nonblank,
};

const CANDIDATE_SET_DOMAIN: &[u8] = b"csa-convergence-candidate-set-v1\0";
const ROOT_CLUSTER_DOMAIN: &[u8] = b"csa-convergence-root-cluster-v1\0";
const ROOT_CLUSTER_SET_DOMAIN: &[u8] = b"csa-convergence-root-cluster-set-v1\0";
const REPAIR_BATCH_DOMAIN: &[u8] = b"csa-convergence-repair-batch-v1\0";
const REPAIR_BATCH_SET_DOMAIN: &[u8] = b"csa-convergence-repair-batch-set-v1\0";
const REPAIR_HANDOFF_DOMAIN: &[u8] = b"csa-convergence-repair-handoff-v1\0";

macro_rules! repair_id {
    ($name:ident, $label:literal) => {
        /// Validated, canonical ULID identifying one immutable repair record.
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name(String);

        impl $name {
            /// Generate a fresh record identifier.
            #[must_use]
            pub fn generate() -> Self {
                Self(Ulid::new().to_string())
            }

            /// Parse and canonicalize a record identifier.
            ///
            /// # Errors
            /// Returns an error when `value` is not a ULID.
            pub fn parse(value: &str) -> Result<Self> {
                let ulid = Ulid::from_string(value).map_err(|error| {
                    anyhow::anyhow!("invalid {} ULID '{value}': {error}", $label)
                })?;
                Ok(Self(ulid.to_string()))
            }

            /// Return the canonical identifier.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = anyhow::Error;

            fn from_str(value: &str) -> Result<Self> {
                Self::parse(value)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(&value).map_err(D::Error::custom)
            }
        }
    };
}

repair_id!(RootClusterId, "root cluster id");
repair_id!(RepairBatchId, "repair batch id");
repair_id!(RepairHandoffId, "repair handoff id");

/// Immutable root-cause cluster over a canonical, complete candidate set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootClusterRecord {
    id: RootClusterId,
    epoch_id: EpochId,
    root_cause_key: String,
    candidate_ids: Vec<CandidateId>,
    candidate_set_digest: Sha256Digest,
    disposition_set_digest: Sha256Digest,
    content_digest: Sha256Digest,
}

impl RootClusterRecord {
    /// Construct a canonical root-cause cluster.
    ///
    /// # Errors
    ///
    /// Returns an error for blank root causes, empty candidate sets, duplicate candidate IDs,
    /// or tampered canonical digests.
    pub fn new(
        epoch_id: EpochId,
        root_cause_key: &str,
        candidate_ids: Vec<CandidateId>,
        disposition_set_digest: Sha256Digest,
    ) -> Result<Self> {
        let candidate_ids = canonical_candidates(candidate_ids)?;
        let root_cause_key = normalize_nonblank("root cluster root cause key", root_cause_key)?;
        let candidate_set_digest = candidate_set_digest(&candidate_ids);
        let content_digest = root_cluster_content_digest(
            &epoch_id,
            &root_cause_key,
            &candidate_set_digest,
            &disposition_set_digest,
        );
        Ok(Self {
            id: RootClusterId::generate(),
            epoch_id,
            root_cause_key,
            candidate_ids,
            candidate_set_digest,
            disposition_set_digest,
            content_digest,
        })
    }

    /// Return the durable cluster identity.
    #[must_use]
    pub fn id(&self) -> &RootClusterId {
        &self.id
    }

    /// Return the immutable epoch containing every clustered candidate.
    #[must_use]
    pub fn epoch_id(&self) -> &EpochId {
        &self.epoch_id
    }

    /// Return the verifier-supplied root-cause key.
    #[must_use]
    pub fn root_cause_key(&self) -> &str {
        &self.root_cause_key
    }

    /// Return lexically sorted, unique cluster members.
    #[must_use]
    pub fn candidate_ids(&self) -> &[CandidateId] {
        &self.candidate_ids
    }

    /// Return the digest of the complete candidate union.
    #[must_use]
    pub fn candidate_set_digest(&self) -> &Sha256Digest {
        &self.candidate_set_digest
    }

    /// Return the digest of the terminal candidate dispositions used to cluster.
    #[must_use]
    pub fn disposition_set_digest(&self) -> &Sha256Digest {
        &self.disposition_set_digest
    }

    /// Return the canonical content digest excluding the generated record ID.
    #[must_use]
    pub fn content_digest(&self) -> &Sha256Digest {
        &self.content_digest
    }

    /// Verify the record was not tampered with after construction.
    ///
    /// # Errors
    ///
    /// Returns an error when canonical member order or any digest differs.
    pub fn validate(&self) -> Result<()> {
        let canonical = canonical_candidates(self.candidate_ids.clone())?;
        if canonical != self.candidate_ids {
            bail!("root cluster {} candidate ids are not canonical", self.id);
        }
        let candidate_digest = candidate_set_digest(&self.candidate_ids);
        if candidate_digest != self.candidate_set_digest {
            bail!("root cluster {} candidate set digest mismatch", self.id);
        }
        let content_digest = root_cluster_content_digest(
            &self.epoch_id,
            &self.root_cause_key,
            &self.candidate_set_digest,
            &self.disposition_set_digest,
        );
        if content_digest != self.content_digest {
            bail!("root cluster {} content digest mismatch", self.id);
        }
        Ok(())
    }

    /// Compute a canonical digest of a complete root-cluster set.
    #[must_use]
    pub fn set_digest(records: &[Self]) -> Sha256Digest {
        let mut digests = records
            .iter()
            .map(|record| record.content_digest.as_str())
            .collect::<Vec<_>>();
        digests.sort_unstable();
        hash_fields(ROOT_CLUSTER_SET_DOMAIN, &digests)
    }
}

/// Immutable consolidated repair batch for exactly one root cluster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairBatchRecord {
    id: RepairBatchId,
    root_cluster_id: RootClusterId,
    epoch_id: EpochId,
    candidate_ids: Vec<CandidateId>,
    candidate_set_digest: Sha256Digest,
    disposition_set_digest: Sha256Digest,
    corrections: Vec<String>,
    regression_tests: Vec<String>,
    docs_contracts: Vec<String>,
    compatibility_migrations: Vec<String>,
    sibling_call_sites: Vec<String>,
    content_digest: Sha256Digest,
}

impl RepairBatchRecord {
    /// Construct one consolidated repair batch, canonically unioning every work category.
    ///
    /// # Errors
    ///
    /// Returns an error for empty or duplicate candidate IDs, or blank/duplicate work items.
    #[expect(
        clippy::too_many_arguments,
        reason = "repair batch content is immutable evidence"
    )]
    pub fn new(
        root_cluster_id: RootClusterId,
        epoch_id: EpochId,
        candidate_ids: Vec<CandidateId>,
        disposition_set_digest: Sha256Digest,
        corrections: Vec<String>,
        regression_tests: Vec<String>,
        docs_contracts: Vec<String>,
        compatibility_migrations: Vec<String>,
        sibling_call_sites: Vec<String>,
    ) -> Result<Self> {
        let candidate_ids = canonical_candidates(candidate_ids)?;
        let corrections = canonical_work_items("repair correction", corrections)?;
        let regression_tests = canonical_work_items("repair regression test", regression_tests)?;
        let docs_contracts =
            canonical_work_items("repair documentation or contract", docs_contracts)?;
        let compatibility_migrations = canonical_work_items(
            "repair compatibility or migration",
            compatibility_migrations,
        )?;
        let sibling_call_sites =
            canonical_work_items("repair sibling call site", sibling_call_sites)?;
        let candidate_set_digest = candidate_set_digest(&candidate_ids);
        let content_digest = repair_batch_content_digest(
            &root_cluster_id,
            &epoch_id,
            &candidate_set_digest,
            &disposition_set_digest,
            &corrections,
            &regression_tests,
            &docs_contracts,
            &compatibility_migrations,
            &sibling_call_sites,
        );
        Ok(Self {
            id: RepairBatchId::generate(),
            root_cluster_id,
            epoch_id,
            candidate_ids,
            candidate_set_digest,
            disposition_set_digest,
            corrections,
            regression_tests,
            docs_contracts,
            compatibility_migrations,
            sibling_call_sites,
            content_digest,
        })
    }

    /// Return the durable batch identity.
    #[must_use]
    pub fn id(&self) -> &RepairBatchId {
        &self.id
    }

    /// Return the sole root cluster represented by this batch.
    #[must_use]
    pub fn root_cluster_id(&self) -> &RootClusterId {
        &self.root_cluster_id
    }

    /// Return the immutable epoch represented by this batch.
    #[must_use]
    pub fn epoch_id(&self) -> &EpochId {
        &self.epoch_id
    }

    /// Return the complete candidate union.
    #[must_use]
    pub fn candidate_ids(&self) -> &[CandidateId] {
        &self.candidate_ids
    }

    /// Return the candidate union digest.
    #[must_use]
    pub fn candidate_set_digest(&self) -> &Sha256Digest {
        &self.candidate_set_digest
    }

    /// Return the terminal disposition-set digest.
    #[must_use]
    pub fn disposition_set_digest(&self) -> &Sha256Digest {
        &self.disposition_set_digest
    }

    /// Return corrections in canonical order.
    #[must_use]
    pub fn corrections(&self) -> &[String] {
        &self.corrections
    }

    /// Return regression-test work in canonical order.
    #[must_use]
    pub fn regression_tests(&self) -> &[String] {
        &self.regression_tests
    }

    /// Return documentation and contract work in canonical order.
    #[must_use]
    pub fn docs_contracts(&self) -> &[String] {
        &self.docs_contracts
    }

    /// Return compatibility and migration work in canonical order.
    #[must_use]
    pub fn compatibility_migrations(&self) -> &[String] {
        &self.compatibility_migrations
    }

    /// Return sibling call-site work in canonical order.
    #[must_use]
    pub fn sibling_call_sites(&self) -> &[String] {
        &self.sibling_call_sites
    }

    /// Return the canonical content digest excluding the generated record ID.
    #[must_use]
    pub fn content_digest(&self) -> &Sha256Digest {
        &self.content_digest
    }

    /// Verify that all derived fields and canonical unions are intact.
    ///
    /// # Errors
    /// Returns an error for altered digests or noncanonical member lists.
    pub fn validate(&self) -> Result<()> {
        if canonical_candidates(self.candidate_ids.clone())? != self.candidate_ids {
            bail!("repair batch {} candidate ids are not canonical", self.id);
        }
        if candidate_set_digest(&self.candidate_ids) != self.candidate_set_digest {
            bail!("repair batch {} candidate set digest mismatch", self.id);
        }
        for (field, items) in [
            ("repair correction", &self.corrections),
            ("repair regression test", &self.regression_tests),
            ("repair documentation or contract", &self.docs_contracts),
            (
                "repair compatibility or migration",
                &self.compatibility_migrations,
            ),
            ("repair sibling call site", &self.sibling_call_sites),
        ] {
            if canonical_work_items(field, items.clone())? != *items {
                bail!("repair batch {} {field} items are not canonical", self.id);
            }
        }
        let content_digest = repair_batch_content_digest(
            &self.root_cluster_id,
            &self.epoch_id,
            &self.candidate_set_digest,
            &self.disposition_set_digest,
            &self.corrections,
            &self.regression_tests,
            &self.docs_contracts,
            &self.compatibility_migrations,
            &self.sibling_call_sites,
        );
        if content_digest != self.content_digest {
            bail!("repair batch {} content digest mismatch", self.id);
        }
        Ok(())
    }

    /// Compute a canonical digest of complete repair batches.
    #[must_use]
    pub fn set_digest(records: &[Self]) -> Sha256Digest {
        let mut digests = records
            .iter()
            .map(|record| record.content_digest.as_str())
            .collect::<Vec<_>>();
        digests.sort_unstable();
        hash_fields(REPAIR_BATCH_SET_DOMAIN, &digests)
    }
}

/// Immutable writer handoff binding the complete repair authority for one batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairHandoffRecord {
    id: RepairHandoffId,
    campaign_id: CampaignId,
    epoch_id: EpochId,
    repair_batch_id: RepairBatchId,
    command_authority_digest: Sha256Digest,
    candidate_set_digest: Sha256Digest,
    disposition_set_digest: Sha256Digest,
    cluster_set_digest: Sha256Digest,
    repair_batch_set_digest: Sha256Digest,
    artifact: ArtifactEvidenceRef,
    content_digest: Sha256Digest,
}

impl RepairHandoffRecord {
    /// Construct an immutable, digest-bound union handoff for exactly one repair batch.
    #[expect(
        clippy::too_many_arguments,
        reason = "all authority evidence must be explicit"
    )]
    #[must_use]
    pub fn new(
        campaign_id: CampaignId,
        epoch_id: EpochId,
        repair_batch_id: RepairBatchId,
        command_authority_digest: Sha256Digest,
        candidate_set_digest: Sha256Digest,
        disposition_set_digest: Sha256Digest,
        cluster_set_digest: Sha256Digest,
        repair_batch_set_digest: Sha256Digest,
        artifact: ArtifactEvidenceRef,
    ) -> Self {
        let content_digest = repair_handoff_content_digest(
            &campaign_id,
            &epoch_id,
            &repair_batch_id,
            &command_authority_digest,
            &candidate_set_digest,
            &disposition_set_digest,
            &cluster_set_digest,
            &repair_batch_set_digest,
            &artifact,
        );
        Self {
            id: RepairHandoffId::generate(),
            campaign_id,
            epoch_id,
            repair_batch_id,
            command_authority_digest,
            candidate_set_digest,
            disposition_set_digest,
            cluster_set_digest,
            repair_batch_set_digest,
            artifact,
            content_digest,
        }
    }

    /// Return the durable handoff identity.
    #[must_use]
    pub fn id(&self) -> &RepairHandoffId {
        &self.id
    }

    /// Return the campaign authorizing this handoff.
    #[must_use]
    pub fn campaign_id(&self) -> &CampaignId {
        &self.campaign_id
    }

    /// Return the frozen epoch the writer must validate.
    #[must_use]
    pub fn epoch_id(&self) -> &EpochId {
        &self.epoch_id
    }

    /// Return the single consolidated repair batch.
    #[must_use]
    pub fn repair_batch_id(&self) -> &RepairBatchId {
        &self.repair_batch_id
    }

    /// Return the frozen command authority digest required before writer launch.
    #[must_use]
    pub fn command_authority_digest(&self) -> &Sha256Digest {
        &self.command_authority_digest
    }

    /// Return the complete candidate-set digest.
    #[must_use]
    pub fn candidate_set_digest(&self) -> &Sha256Digest {
        &self.candidate_set_digest
    }

    /// Return the complete terminal-disposition-set digest.
    #[must_use]
    pub fn disposition_set_digest(&self) -> &Sha256Digest {
        &self.disposition_set_digest
    }

    /// Return the complete root-cluster-set digest.
    #[must_use]
    pub fn cluster_set_digest(&self) -> &Sha256Digest {
        &self.cluster_set_digest
    }

    /// Return the complete repair-batch-set digest.
    #[must_use]
    pub fn repair_batch_set_digest(&self) -> &Sha256Digest {
        &self.repair_batch_set_digest
    }

    /// Return the immutable union artifact reference.
    #[must_use]
    pub fn artifact(&self) -> &ArtifactEvidenceRef {
        &self.artifact
    }

    /// Return the canonical content digest excluding the generated record ID.
    #[must_use]
    pub fn content_digest(&self) -> &Sha256Digest {
        &self.content_digest
    }

    /// Verify all immutable handoff bindings.
    ///
    /// # Errors
    /// Returns an error if any authority, set, artifact, or content digest was altered.
    pub fn validate(&self) -> Result<()> {
        let content_digest = repair_handoff_content_digest(
            &self.campaign_id,
            &self.epoch_id,
            &self.repair_batch_id,
            &self.command_authority_digest,
            &self.candidate_set_digest,
            &self.disposition_set_digest,
            &self.cluster_set_digest,
            &self.repair_batch_set_digest,
            &self.artifact,
        );
        if content_digest != self.content_digest {
            bail!("repair handoff {} content digest mismatch", self.id);
        }
        Ok(())
    }
}

fn canonical_candidates(mut candidate_ids: Vec<CandidateId>) -> Result<Vec<CandidateId>> {
    if candidate_ids.is_empty() {
        bail!("candidate set must not be empty");
    }
    candidate_ids.sort_unstable_by(|left, right| left.as_str().cmp(right.as_str()));
    if candidate_ids
        .windows(2)
        .any(|pair| pair[0].as_str() == pair[1].as_str())
    {
        bail!("candidate set contains duplicate candidate ids");
    }
    Ok(candidate_ids)
}

fn canonical_work_items(field: &str, items: Vec<String>) -> Result<Vec<String>> {
    let mut canonical = items
        .into_iter()
        .map(|item| normalize_nonblank(field, &item))
        .collect::<Result<Vec<_>>>()?;
    canonical.sort_unstable();
    if canonical.windows(2).any(|pair| pair[0] == pair[1]) {
        bail!("{field} set contains duplicate items");
    }
    Ok(canonical)
}

fn candidate_set_digest(candidate_ids: &[CandidateId]) -> Sha256Digest {
    hash_fields(
        CANDIDATE_SET_DOMAIN,
        &candidate_ids
            .iter()
            .map(CandidateId::as_str)
            .collect::<Vec<_>>(),
    )
}

fn root_cluster_content_digest(
    epoch_id: &EpochId,
    root_cause_key: &str,
    candidate_set_digest: &Sha256Digest,
    disposition_set_digest: &Sha256Digest,
) -> Sha256Digest {
    hash_fields(
        ROOT_CLUSTER_DOMAIN,
        &[
            epoch_id.as_str(),
            root_cause_key,
            candidate_set_digest.as_str(),
            disposition_set_digest.as_str(),
        ],
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "the entire repair union is digest-bound"
)]
fn repair_batch_content_digest(
    root_cluster_id: &RootClusterId,
    epoch_id: &EpochId,
    candidate_set_digest: &Sha256Digest,
    disposition_set_digest: &Sha256Digest,
    corrections: &[String],
    regression_tests: &[String],
    docs_contracts: &[String],
    compatibility_migrations: &[String],
    sibling_call_sites: &[String],
) -> Sha256Digest {
    let mut fields = vec![
        root_cluster_id.as_str(),
        epoch_id.as_str(),
        candidate_set_digest.as_str(),
        disposition_set_digest.as_str(),
    ];
    for (category, items) in [
        ("correction", corrections),
        ("regression_test", regression_tests),
        ("docs_contract", docs_contracts),
        ("compatibility_migration", compatibility_migrations),
        ("sibling_call_site", sibling_call_sites),
    ] {
        fields.push(category);
        fields.extend(items.iter().map(String::as_str));
    }
    hash_fields(REPAIR_BATCH_DOMAIN, &fields)
}

#[expect(
    clippy::too_many_arguments,
    reason = "the entire repair authority is digest-bound"
)]
fn repair_handoff_content_digest(
    campaign_id: &CampaignId,
    epoch_id: &EpochId,
    repair_batch_id: &RepairBatchId,
    command_authority_digest: &Sha256Digest,
    candidate_set_digest: &Sha256Digest,
    disposition_set_digest: &Sha256Digest,
    cluster_set_digest: &Sha256Digest,
    repair_batch_set_digest: &Sha256Digest,
    artifact: &ArtifactEvidenceRef,
) -> Sha256Digest {
    hash_fields(
        REPAIR_HANDOFF_DOMAIN,
        &[
            campaign_id.as_str(),
            epoch_id.as_str(),
            repair_batch_id.as_str(),
            command_authority_digest.as_str(),
            candidate_set_digest.as_str(),
            disposition_set_digest.as_str(),
            cluster_set_digest.as_str(),
            repair_batch_set_digest.as_str(),
            artifact.csa_session_id().as_str(),
            artifact.path().as_str(),
            artifact.digest().as_str(),
        ],
    )
}
