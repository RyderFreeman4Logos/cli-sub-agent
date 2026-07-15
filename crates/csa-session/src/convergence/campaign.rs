use chrono::{DateTime, Utc};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};

use super::{CampaignId, CampaignRecord, CommandAuthoritySnapshot, Sha256Digest};

impl<'de> Deserialize<'de> for CampaignRecord {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawCampaignRecord {
            id: CampaignId,
            created_at: DateTime<Utc>,
            policy_digest: Option<Sha256Digest>,
            command_authority: CommandAuthoritySnapshot,
            command_authority_digest: Sha256Digest,
        }

        let raw = RawCampaignRecord::deserialize(deserializer)?;
        let expected = raw.command_authority.digest();
        if raw.command_authority_digest != expected {
            return Err(D::Error::custom(format!(
                "command authority digest mismatch: stored {}, recomputed {}",
                raw.command_authority_digest, expected
            )));
        }
        Ok(Self {
            id: raw.id,
            created_at: raw.created_at,
            policy_digest: raw.policy_digest,
            command_authority: raw.command_authority,
            command_authority_digest: raw.command_authority_digest,
        })
    }
}
