use anyhow::Result;
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

/// Parse a `spec.toml` document after rejecting known non-TOML artifact shapes.
///
/// Generated plans pass through session-output files before they are persisted.
/// If a producing step writes a CSA return packet or markdown fence into
/// `spec.toml`, plain TOML parsing reports a low-level syntax error that hides
/// the real contract breach. This parser keeps normal TOML errors intact while
/// failing early for those artifact-shape mismatches.
pub fn parse_spec_document(content: &str, source: &str) -> Result<SpecDocument> {
    reject_non_toml_artifact_shape(content, source)?;
    toml::from_str(content)
        .map_err(|e| anyhow::anyhow!("failed to parse spec file '{}': {}", source, e))
}

fn reject_non_toml_artifact_shape(content: &str, source: &str) -> Result<()> {
    let Some((line_number, line)) = content
        .lines()
        .enumerate()
        .find(|(_, line)| !line.trim().is_empty())
    else {
        return Ok(());
    };

    let trimmed = line
        .trim_start()
        .trim_start_matches('\u{feff}')
        .trim_start();
    let lower_trimmed = trimmed.to_ascii_lowercase();
    let marker_kind = if trimmed.starts_with("<!-- CSA:SECTION:") {
        Some("a CSA section marker")
    } else if trimmed.starts_with("<!--") {
        Some("an HTML marker")
    } else if lower_trimmed.starts_with("<!doctype html") || lower_trimmed.starts_with("<html") {
        Some("an HTML document marker")
    } else if trimmed.starts_with("```") {
        Some("a Markdown code fence")
    } else {
        None
    };

    if let Some(marker_kind) = marker_kind {
        let redacted_marker = csa_session::redact_text_content(line.trim());
        anyhow::bail!(
            "spec artifact-shape error in '{}': expected TOML spec.toml, but line {} starts \
             with {}: `{}`. This usually means a CSA return packet, markdown output, or \
             upstream MCP/tool error was written to spec.toml; inspect the producing step \
             before running `csa todo persist`.",
            source,
            line_number + 1,
            marker_kind,
            redacted_marker
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{CriterionKind, CriterionStatus, SpecCriterion, SpecDocument, parse_spec_document};

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

    #[test]
    fn parse_spec_document_rejects_csa_section_marker_artifact_shape() {
        let err = parse_spec_document(
            "<!-- CSA:SECTION:summary -->\nnot a toml spec\n<!-- CSA:SECTION:summary:END -->\n",
            "output/mktd-save/spec.toml",
        )
        .expect_err("CSA section marker must be rejected before TOML parsing");
        let message = err.to_string();

        assert!(message.contains("spec artifact-shape error"));
        assert!(message.contains("CSA section marker"));
        assert!(message.contains("output/mktd-save/spec.toml"));
        assert!(!message.contains("not a toml spec"));
        assert!(message.contains("inspect the producing step"));
    }

    #[test]
    fn parse_spec_document_rejects_markdown_fence_artifact_shape() {
        let err = parse_spec_document(
            "```toml\nschema_version = 1\n```\n",
            "output/mktd-save/spec.toml",
        )
        .expect_err("markdown-fenced spec artifact must be rejected before TOML parsing");
        let message = err.to_string();

        assert!(message.contains("spec artifact-shape error"));
        assert!(message.contains("Markdown code fence"));
    }

    #[test]
    fn parse_spec_document_rejects_html_document_artifact_shape() {
        let err = parse_spec_document(
            "<!doctype html>\n<html><body>mempal MCP schema mismatch</body></html>\n",
            "output/mktd-save/spec.toml",
        )
        .expect_err("HTML spec artifact must be rejected before TOML parsing");
        let message = err.to_string();

        assert!(message.contains("spec artifact-shape error"));
        assert!(message.contains("HTML document marker"));
        assert!(!message.contains("mempal MCP schema mismatch"));
    }

    #[test]
    fn parse_spec_document_redacts_secret_like_marker_line() {
        let err = parse_spec_document(
            "<!-- CSA:SECTION:summary --> api_key=fixture12345\nAuthorization: Bearer fixturebearertoken\n",
            "output/mktd-save/spec.toml",
        )
        .expect_err("CSA section marker must be rejected before TOML parsing");
        let message = err.to_string();

        assert!(message.contains("spec artifact-shape error"));
        assert!(message.contains("[REDACTED]"));
        assert!(!message.contains("fixture12345"));
        assert!(!message.contains("fixturebearertoken"));
        assert!(!message.contains("Authorization: Bearer"));
    }
}
