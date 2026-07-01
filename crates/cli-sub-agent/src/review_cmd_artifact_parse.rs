use anyhow::{Context, Result};
use csa_session::{Finding, SeveritySummary};
use serde_json::Value;

#[derive(Debug)]
pub(crate) struct LossyReviewArtifactFields {
    pub(crate) findings: Vec<Finding>,
    pub(crate) severity_summary: SeveritySummary,
    pub(crate) overall_risk: Option<String>,
    pub(crate) schema_version: Option<String>,
    pub(crate) bug_category_checklist: Vec<Value>,
}

pub(crate) fn parse_review_artifact_fields_lossy(
    content: &str,
) -> Result<LossyReviewArtifactFields> {
    let value: Value = serde_json::from_str(content).context("parse review artifact JSON")?;
    let findings = value
        .get("findings")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| serde_json::from_value::<Finding>(item.clone()).ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut severity_summary = value
        .get("severity_summary")
        .and_then(|summary| serde_json::from_value::<SeveritySummary>(summary.clone()).ok())
        .unwrap_or_else(|| SeveritySummary::from_findings(&findings));
    if severity_summary_is_zero(&severity_summary) && !findings.is_empty() {
        severity_summary = SeveritySummary::from_findings(&findings);
    }
    let overall_risk = value
        .get("overall_risk")
        .and_then(Value::as_str)
        .map(str::to_string);
    let schema_version = value
        .get("schema_version")
        .and_then(Value::as_str)
        .map(str::to_string);
    let bug_category_checklist = value
        .get("bug_category_checklist")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    Ok(LossyReviewArtifactFields {
        findings,
        severity_summary,
        overall_risk,
        schema_version,
        bug_category_checklist,
    })
}

fn severity_summary_is_zero(summary: &SeveritySummary) -> bool {
    summary.critical == 0 && summary.high == 0 && summary.medium == 0 && summary.low == 0
}
