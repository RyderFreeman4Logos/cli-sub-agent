use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

fn default_schema_version() -> String {
    "1.0".to_string()
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    #[serde(rename = "critical")]
    Critical = 5,
    #[serde(rename = "high")]
    High = 4,
    #[serde(rename = "medium")]
    Medium = 3,
    #[serde(rename = "low")]
    Low = 2,
    #[serde(rename = "info")]
    Info = 1,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    pub fid: String,
    pub file: String,
    #[serde(default)]
    pub line: Option<u32>,
    pub rule_id: String,
    pub summary: String,
    pub engine: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct SeveritySummary {
    #[serde(default)]
    pub critical: u32,
    #[serde(default)]
    pub high: u32,
    #[serde(default)]
    pub medium: u32,
    #[serde(default)]
    pub low: u32,
    #[serde(default)]
    pub info: u32,
}

impl SeveritySummary {
    pub fn from_findings(findings: &[Finding]) -> Self {
        let mut summary = Self::default();
        for finding in findings {
            match finding.severity {
                Severity::Critical => summary.critical += 1,
                Severity::High => summary.high += 1,
                Severity::Medium => summary.medium += 1,
                Severity::Low => summary.low += 1,
                Severity::Info => summary.info += 1,
            }
        }
        summary
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ReviewArtifact {
    #[serde(default)]
    pub findings: Vec<Finding>,
    #[serde(default)]
    pub severity_summary: SeveritySummary,
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
    pub session_id: String,
    pub timestamp: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::{Finding, ReviewArtifact, Severity, SeveritySummary};
    use chrono::Utc;

    fn sample_findings() -> Vec<Finding> {
        vec![
            Finding {
                severity: Severity::Critical,
                fid: "FIDCRIT".to_string(),
                file: "src/a.rs".to_string(),
                line: Some(10),
                rule_id: "rule.critical".to_string(),
                summary: "critical summary".to_string(),
                engine: "semgrep".to_string(),
            },
            Finding {
                severity: Severity::High,
                fid: "FIDHIGH".to_string(),
                file: "src/b.rs".to_string(),
                line: Some(20),
                rule_id: "rule.high".to_string(),
                summary: "high summary".to_string(),
                engine: "clippy".to_string(),
            },
            Finding {
                severity: Severity::Medium,
                fid: "FIDMED".to_string(),
                file: "src/c.rs".to_string(),
                line: None,
                rule_id: "rule.medium".to_string(),
                summary: "medium summary".to_string(),
                engine: "reviewer".to_string(),
            },
            Finding {
                severity: Severity::Low,
                fid: "FIDLOW".to_string(),
                file: "src/d.rs".to_string(),
                line: Some(1),
                rule_id: "rule.low".to_string(),
                summary: "low summary".to_string(),
                engine: "reviewer".to_string(),
            },
            Finding {
                severity: Severity::Info,
                fid: "FIDINFO".to_string(),
                file: "src/e.rs".to_string(),
                line: None,
                rule_id: "rule.info".to_string(),
                summary: "info summary".to_string(),
                engine: "reviewer".to_string(),
            },
        ]
    }

    #[test]
    fn test_severity_ordering() {
        assert!(Severity::Critical > Severity::High);
        assert!(Severity::High > Severity::Medium);
        assert!(Severity::Medium > Severity::Low);
        assert!(Severity::Low > Severity::Info);
    }

    #[test]
    fn test_finding_serde_roundtrip() {
        let finding = Finding {
            severity: Severity::High,
            fid: "FINDINGID1234567890ABCDEFGH".to_string(),
            file: "src/lib.rs".to_string(),
            line: Some(42),
            rule_id: "rust.no-unwrap".to_string(),
            summary: "avoid unwrap in production code".to_string(),
            engine: "semgrep".to_string(),
        };

        let json = serde_json::to_string(&finding).expect("finding serialize should succeed");
        let decoded: Finding =
            serde_json::from_str(&json).expect("finding deserialize should succeed");

        assert_eq!(decoded, finding);
    }

    #[test]
    fn test_review_artifact_serde_roundtrip() {
        let findings = sample_findings();
        let severity_summary = SeveritySummary::from_findings(&findings);
        let artifact = ReviewArtifact {
            findings,
            severity_summary,
            schema_version: "1.0".to_string(),
            session_id: "01JABCDEF0123456789ABCDEFG".to_string(),
            timestamp: Utc::now(),
        };

        let json = serde_json::to_string(&artifact).expect("artifact serialize should succeed");
        let decoded: ReviewArtifact =
            serde_json::from_str(&json).expect("artifact deserialize should succeed");

        assert_eq!(decoded, artifact);
    }

    #[test]
    fn test_severity_summary_from_findings_counts_correctly() {
        let findings = sample_findings();
        let summary = SeveritySummary::from_findings(&findings);

        assert_eq!(summary.critical, 1);
        assert_eq!(summary.high, 1);
        assert_eq!(summary.medium, 1);
        assert_eq!(summary.low, 1);
        assert_eq!(summary.info, 1);
    }

    #[test]
    fn test_schema_version_defaults_to_1_0() {
        let json = r#"
        {
            "findings": [],
            "severity_summary": { "critical": 0, "high": 0, "medium": 0, "low": 0, "info": 0 },
            "session_id": "01JABCDEF0123456789ABCDEFG",
            "timestamp": "2026-02-24T00:00:00Z"
        }
        "#;

        let artifact: ReviewArtifact =
            serde_json::from_str(json).expect("deserialize with missing schema_version");

        assert_eq!(artifact.schema_version, "1.0");
    }

    #[test]
    fn test_empty_findings_produces_zero_summary() {
        let findings: Vec<Finding> = Vec::new();
        let summary = SeveritySummary::from_findings(&findings);

        assert_eq!(summary.critical, 0);
        assert_eq!(summary.high, 0);
        assert_eq!(summary.medium, 0);
        assert_eq!(summary.low, 0);
        assert_eq!(summary.info, 0);
    }
}
