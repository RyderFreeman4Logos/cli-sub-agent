use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    #[default]
    Unchecked,
    Checked,
    Failed,
    #[serde(rename = "na")]
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecklistItem {
    pub id: String,
    pub source: String,
    pub description: String,
    #[serde(default)]
    pub status: CheckStatus,
    #[serde(default)]
    pub evidence: String,
    #[serde(default)]
    pub reviewer: String,
    #[serde(default)]
    pub checked_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecklistMeta {
    pub project_root: String,
    pub branch: String,
    pub created_at: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub profile: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecklistDocument {
    pub meta: ChecklistMeta,
    #[serde(default)]
    pub criteria: Vec<ChecklistItem>,
}

impl ChecklistDocument {
    pub fn all_checked(&self) -> bool {
        self.criteria
            .iter()
            .all(|c| matches!(c.status, CheckStatus::Checked | CheckStatus::NotApplicable))
    }

    pub fn summary(&self) -> ChecklistSummary {
        let mut s = ChecklistSummary::default();
        for c in &self.criteria {
            match c.status {
                CheckStatus::Unchecked => s.unchecked += 1,
                CheckStatus::Checked => s.checked += 1,
                CheckStatus::Failed => s.failed += 1,
                CheckStatus::NotApplicable => s.not_applicable += 1,
            }
        }
        s
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ChecklistSummary {
    pub unchecked: usize,
    pub checked: usize,
    pub failed: usize,
    pub not_applicable: usize,
}

#[cfg(test)]
mod tests {
    use super::{CheckStatus, ChecklistDocument, ChecklistItem, ChecklistMeta, ChecklistSummary};

    fn item(id: &str, status: CheckStatus) -> ChecklistItem {
        ChecklistItem {
            id: id.to_string(),
            source: "Rust 002".to_string(),
            description: "Propagate errors without unwrap".to_string(),
            status,
            evidence: String::new(),
            reviewer: String::new(),
            checked_at: String::new(),
        }
    }

    fn doc(criteria: Vec<ChecklistItem>) -> ChecklistDocument {
        ChecklistDocument {
            meta: ChecklistMeta {
                project_root: "/repo".to_string(),
                branch: "feature".to_string(),
                created_at: "2026-04-28T00:00:00Z".to_string(),
                scope: "base:main".to_string(),
                profile: "rust".to_string(),
            },
            criteria,
        }
    }

    #[test]
    fn all_checked_accepts_checked_and_not_applicable_only() {
        assert!(
            doc(vec![
                item("a", CheckStatus::Checked),
                item("b", CheckStatus::NotApplicable),
            ])
            .all_checked()
        );

        assert!(
            !doc(vec![
                item("a", CheckStatus::Checked),
                item("b", CheckStatus::Unchecked),
            ])
            .all_checked()
        );

        assert!(!doc(vec![item("a", CheckStatus::Failed)]).all_checked());
    }

    #[test]
    fn summary_counts_each_status() {
        let summary = doc(vec![
            item("a", CheckStatus::Unchecked),
            item("b", CheckStatus::Checked),
            item("c", CheckStatus::Failed),
            item("d", CheckStatus::NotApplicable),
            item("e", CheckStatus::Checked),
        ])
        .summary();

        assert_eq!(
            summary,
            ChecklistSummary {
                unchecked: 1,
                checked: 2,
                failed: 1,
                not_applicable: 1,
            }
        );
    }

    #[test]
    fn toml_roundtrip_preserves_document() {
        let original = doc(vec![item("rust-002", CheckStatus::Checked)]);
        let encoded = toml::to_string_pretty(&original).expect("serialize checklist");
        assert!(encoded.contains("status = \"checked\""));

        let decoded: ChecklistDocument = toml::from_str(&encoded).expect("deserialize checklist");
        assert_eq!(decoded, original);
    }
}
