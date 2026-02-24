use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Temporary exception allowing hook failures in closed mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waiver {
    pub scope: String,
    pub justification: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approver: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

impl Waiver {
    /// Returns true when waiver is active (not expired) or has no expiration.
    pub fn is_valid(&self) -> bool {
        self.expires_at.is_none_or(|expires_at| Utc::now() <= expires_at)
    }
}

/// Collection helper for waiver checks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WaiverSet(pub Vec<Waiver>);

impl WaiverSet {
    pub fn has_valid_waiver(&self) -> bool {
        self.0.iter().any(Waiver::is_valid)
    }
}

impl From<Vec<Waiver>> for WaiverSet {
    fn from(value: Vec<Waiver>) -> Self {
        Self(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn waiver_with_expiry(expires_at: Option<DateTime<Utc>>) -> Waiver {
        Waiver {
            scope: "pre_run".to_string(),
            justification: "temporary exception".to_string(),
            ticket: Some("CSA-123".to_string()),
            approver: Some("reviewer".to_string()),
            expires_at,
        }
    }

    #[test]
    fn test_waiver_validity_expired_vs_active() {
        let expired = waiver_with_expiry(Some(Utc::now() - Duration::minutes(1)));
        let active = waiver_with_expiry(Some(Utc::now() + Duration::minutes(1)));
        let no_expiry = waiver_with_expiry(None);

        assert!(!expired.is_valid());
        assert!(active.is_valid());
        assert!(no_expiry.is_valid());
    }

    #[test]
    fn test_waiver_set_has_valid_waiver() {
        let waivers = WaiverSet(vec![
            waiver_with_expiry(Some(Utc::now() - Duration::minutes(2))),
            waiver_with_expiry(Some(Utc::now() + Duration::minutes(2))),
        ]);
        assert!(waivers.has_valid_waiver());

        let expired_only = WaiverSet(vec![waiver_with_expiry(Some(
            Utc::now() - Duration::minutes(2),
        ))]);
        assert!(!expired_only.has_valid_waiver());
    }
}
