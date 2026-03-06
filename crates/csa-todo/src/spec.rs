use serde::{Deserialize, Serialize};

fn default_schema_version() -> u32 {
    1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CriterionKind {
    Scenario,
    Property,
    Check,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CriterionStatus {
    #[default]
    Pending,
    Verified,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecCriterion {
    pub kind: CriterionKind,
    pub id: String,
    pub description: String,
    #[serde(default)]
    pub status: CriterionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecDocument {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub plan_ulid: String,
    pub summary: String,
    #[serde(default)]
    pub criteria: Vec<SpecCriterion>,
}

impl Default for SpecDocument {
    fn default() -> Self {
        Self {
            schema_version: default_schema_version(),
            plan_ulid: String::new(),
            summary: String::new(),
            criteria: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CriterionKind, CriterionStatus, SpecCriterion, SpecDocument};

    fn sample_criterion() -> SpecCriterion {
        SpecCriterion {
            kind: CriterionKind::Scenario,
            id: "scenario-login-success".to_string(),
            description: "Successful login persists the authenticated session.".to_string(),
            status: CriterionStatus::Verified,
        }
    }

    #[test]
    fn spec_criterion_toml_roundtrip() {
        let criterion = sample_criterion();

        let toml = toml::to_string(&criterion).expect("criterion should serialize to TOML");
        let decoded: SpecCriterion =
            toml::from_str(&toml).expect("criterion should deserialize from TOML");

        assert_eq!(decoded, criterion);
    }

    #[test]
    fn spec_criterion_status_defaults_to_pending() {
        let toml = r#"
kind = "check"
id = "check-empty"
description = "Missing status should default to pending."
"#;

        let decoded: SpecCriterion =
            toml::from_str(toml).expect("criterion should deserialize with default status");

        assert_eq!(decoded.status, CriterionStatus::Pending);
    }

    #[test]
    fn spec_document_toml_roundtrip() {
        let document = SpecDocument {
            schema_version: 1,
            plan_ulid: "01JABCDEF0123456789ABCDEFG".to_string(),
            summary: "Specification for validating spec.toml persistence.".to_string(),
            criteria: vec![
                sample_criterion(),
                SpecCriterion {
                    kind: CriterionKind::Property,
                    id: "property-roundtrip".to_string(),
                    description: "Roundtrip serialization preserves all criterion fields."
                        .to_string(),
                    status: CriterionStatus::Pending,
                },
            ],
        };

        let toml = toml::to_string_pretty(&document).expect("document should serialize to TOML");
        let decoded: SpecDocument =
            toml::from_str(&toml).expect("document should deserialize from TOML");

        assert_eq!(decoded, document);
    }
}
