use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

fn default_schema_version() -> String {
    "1.0".to_string()
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Accepted,
    Rejected,
    Deferred,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AdjudicationRecord {
    pub finding_fid: String,
    pub verdict: Verdict,
    pub rationale: String,
    pub reviewer: Option<String>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AdjudicationSet {
    #[serde(default)]
    pub records: Vec<AdjudicationRecord>,
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
}

impl AdjudicationSet {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            schema_version: default_schema_version(),
        }
    }

    pub fn add(&mut self, record: AdjudicationRecord) {
        self.records.push(record);
    }

    pub fn verdict_for(&self, fid: &str) -> Option<&Verdict> {
        self.records
            .iter()
            .rev()
            .find(|record| record.finding_fid == fid)
            .map(|record| &record.verdict)
    }
}

#[cfg(test)]
mod tests {
    use super::{AdjudicationRecord, AdjudicationSet, Verdict};
    use chrono::Utc;

    #[test]
    fn test_verdict_serde_roundtrip_all_variants() {
        let cases = [Verdict::Accepted, Verdict::Rejected, Verdict::Deferred];

        for verdict in cases {
            let json = serde_json::to_string(&verdict).expect("verdict serialize should succeed");
            let decoded: Verdict =
                serde_json::from_str(&json).expect("verdict deserialize should succeed");
            assert_eq!(decoded, verdict);
        }
    }

    #[test]
    fn test_adjudication_record_serde_roundtrip() {
        let record = AdjudicationRecord {
            finding_fid: "FINDINGID1234567890ABCDEFGH".to_string(),
            verdict: Verdict::Accepted,
            rationale: "false positive due to generated code".to_string(),
            reviewer: Some("codex".to_string()),
            timestamp: Utc::now(),
        };

        let json = serde_json::to_string(&record).expect("record serialize should succeed");
        let decoded: AdjudicationRecord =
            serde_json::from_str(&json).expect("record deserialize should succeed");

        assert_eq!(decoded.finding_fid, record.finding_fid);
        assert_eq!(decoded.verdict, record.verdict);
        assert_eq!(decoded.rationale, record.rationale);
        assert_eq!(decoded.reviewer, record.reviewer);
        assert_eq!(decoded.timestamp, record.timestamp);
    }

    #[test]
    fn test_adjudication_set_new_defaults() {
        let set = AdjudicationSet::new();

        assert!(set.records.is_empty());
        assert_eq!(set.schema_version, "1.0");
    }

    #[test]
    fn test_add_and_verdict_for_returns_latest_verdict() {
        let mut set = AdjudicationSet::new();
        let fid = "FID-ALPHA";

        set.add(AdjudicationRecord {
            finding_fid: fid.to_string(),
            verdict: Verdict::Accepted,
            rationale: "initial decision".to_string(),
            reviewer: Some("engine-a".to_string()),
            timestamp: Utc::now(),
        });

        set.add(AdjudicationRecord {
            finding_fid: fid.to_string(),
            verdict: Verdict::Rejected,
            rationale: "re-reviewed decision".to_string(),
            reviewer: Some("engine-b".to_string()),
            timestamp: Utc::now(),
        });

        assert_eq!(set.verdict_for(fid), Some(&Verdict::Rejected));
    }

    #[test]
    fn test_verdict_for_returns_none_for_unknown_fid() {
        let set = AdjudicationSet::new();

        assert_eq!(set.verdict_for("UNKNOWN-FID"), None);
    }

    #[test]
    fn test_multiple_records_same_fid_returns_last_added() {
        let mut set = AdjudicationSet::new();
        let fid = "FID-BETA";

        set.add(AdjudicationRecord {
            finding_fid: fid.to_string(),
            verdict: Verdict::Deferred,
            rationale: "needs more context".to_string(),
            reviewer: Some("engine-a".to_string()),
            timestamp: Utc::now(),
        });
        set.add(AdjudicationRecord {
            finding_fid: fid.to_string(),
            verdict: Verdict::Accepted,
            rationale: "context confirmed".to_string(),
            reviewer: Some("engine-a".to_string()),
            timestamp: Utc::now(),
        });
        set.add(AdjudicationRecord {
            finding_fid: fid.to_string(),
            verdict: Verdict::Rejected,
            rationale: "final override".to_string(),
            reviewer: Some("engine-c".to_string()),
            timestamp: Utc::now(),
        });

        assert_eq!(set.verdict_for(fid), Some(&Verdict::Rejected));
    }
}
