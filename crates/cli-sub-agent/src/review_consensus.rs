use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use csa_config::ProjectConfig;
use csa_config::global::GlobalConfig;
use csa_core::consensus::{
    AgentResponse, ConsensusResult, ConsensusStrategy, resolve_majority, resolve_unanimous,
    resolve_weighted,
};
use csa_core::types::ToolName;
use csa_session::review_artifact::{Finding, ReviewArtifact, SeveritySummary};

pub(crate) const CLEAN: &str = "CLEAN";
pub(crate) const HAS_ISSUES: &str = "HAS_ISSUES";

/// Contract alignment rule_id constants for spec-aware review findings.
/// These are emitted by the review agent when a spec/TODO context is provided.
pub(crate) const RULE_SPEC_DEVIATION: &str = "spec-deviation";
pub(crate) const RULE_UNVERIFIED_CRITERION: &str = "unverified-criterion";
pub(crate) const RULE_BOUNDARY_VIOLATION: &str = "boundary-violation";

pub(crate) fn build_reviewer_tools(
    explicit_tool: Option<ToolName>,
    primary_tool: ToolName,
    project_config: Option<&ProjectConfig>,
    global_config: Option<&GlobalConfig>,
    reviewer_count: usize,
) -> Vec<ToolName> {
    if reviewer_count == 0 {
        return Vec::new();
    }
    if explicit_tool.is_some() {
        return vec![primary_tool; reviewer_count];
    }

    let enabled_tools: Vec<ToolName> = if let Some(cfg) = project_config {
        let tools: Vec<_> = csa_config::global::all_known_tools()
            .iter()
            .filter(|t| cfg.is_tool_auto_selectable(t.as_str()))
            .copied()
            .collect();
        if let Some(gc) = global_config {
            csa_config::global::sort_tools_by_effective_priority(&tools, project_config, gc)
        } else {
            tools
        }
    } else if let Some(gc) = global_config {
        csa_config::global::sort_tools_by_effective_priority(
            csa_config::global::all_known_tools(),
            project_config,
            gc,
        )
    } else {
        csa_config::global::all_known_tools().to_vec()
    };

    let mut pool = vec![primary_tool];
    for tool in enabled_tools {
        if !pool.contains(&tool) {
            pool.push(tool);
        }
    }

    (0..reviewer_count)
        .map(|idx| pool[idx % pool.len()])
        .collect()
}

pub(crate) fn build_multi_reviewer_instruction(
    base_prompt: &str,
    reviewer_index: usize,
    tool: ToolName,
) -> String {
    let output_dir = format!("$CSA_SESSION_DIR/reviewer-{reviewer_index}");
    format!(
        "{base_prompt}\n\
You are reviewer {reviewer_index}. Emit exactly one final verdict token: \
PASS, FAIL, SKIP, or UNCERTAIN.\n\
Legacy aliases accepted: {CLEAN} → PASS, {HAS_ISSUES} → FAIL.\n\
Write review artifacts to {output_dir}/review-findings.json and {output_dir}/review-report.md.\n\
Verdict rules:\n\
- PASS: no serious issues (P0/P1).\n\
- FAIL: serious issues found.\n\
- SKIP: review scope is empty or not applicable.\n\
- UNCERTAIN: insufficient context to make a confident determination.\n\
When spec/contract context is provided, use rule_id values: \
'{RULE_SPEC_DEVIATION}' (implementation contradicts spec), \
'{RULE_UNVERIFIED_CRITERION}' (criterion not addressed by diff), \
'{RULE_BOUNDARY_VIOLATION}' (change exceeds spec scope).\n\
Reviewer tool hint: {}.",
        tool.as_str()
    )
}

pub(crate) fn parse_consensus_strategy(raw: &str) -> Result<ConsensusStrategy> {
    match raw {
        "majority" => Ok(ConsensusStrategy::Majority),
        "weighted" => Ok(ConsensusStrategy::Weighted),
        "unanimous" => Ok(ConsensusStrategy::Unanimous),
        _ => anyhow::bail!(
            "Invalid consensus strategy '{raw}'. Supported values: majority, weighted, unanimous."
        ),
    }
}

pub(crate) fn resolve_consensus(
    strategy: ConsensusStrategy,
    responses: &[AgentResponse],
) -> ConsensusResult {
    match strategy {
        ConsensusStrategy::Majority => resolve_majority(responses),
        ConsensusStrategy::Weighted => resolve_weighted(responses),
        ConsensusStrategy::Unanimous => resolve_unanimous(responses),
        ConsensusStrategy::HumanInTheLoop => {
            unreachable!("human-in-the-loop is not exposed by CLI")
        }
    }
}

pub(crate) fn parse_review_verdict(output: &str, exit_code: i32) -> &'static str {
    let has_issues = contains_verdict_token(output, HAS_ISSUES);
    let clean = contains_verdict_token(output, CLEAN);

    if has_issues {
        HAS_ISSUES
    } else if clean || exit_code == 0 {
        CLEAN
    } else {
        HAS_ISSUES
    }
}

/// Parse review output into four-value `ReviewDecision`.
///
/// Token priority: FAIL/HAS_ISSUES > UNCERTAIN > PASS/CLEAN > SKIP.
/// Falls back to exit code when no verdict token is found.
pub(crate) fn parse_review_decision(
    output: &str,
    exit_code: i32,
) -> csa_core::types::ReviewDecision {
    use csa_core::types::ReviewDecision;

    let has_fail =
        contains_verdict_token(output, HAS_ISSUES) || contains_verdict_token(output, "FAIL");
    let has_uncertain = contains_verdict_token(output, "UNCERTAIN");
    let has_pass = contains_verdict_token(output, CLEAN) || contains_verdict_token(output, "PASS");
    let has_skip = contains_verdict_token(output, "SKIP");

    if has_fail {
        ReviewDecision::Fail
    } else if has_uncertain {
        ReviewDecision::Uncertain
    } else if has_pass {
        ReviewDecision::Pass
    } else if has_skip {
        ReviewDecision::Skip
    } else if exit_code == 0 {
        ReviewDecision::Pass
    } else {
        ReviewDecision::Fail
    }
}

fn contains_verdict_token(haystack: &str, token: &str) -> bool {
    haystack
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|part| part.eq_ignore_ascii_case(token))
}

pub(crate) fn consensus_verdict(consensus_result: &ConsensusResult) -> &'static str {
    if let Some(decision) = &consensus_result.decision {
        if decision.eq_ignore_ascii_case(CLEAN) {
            return CLEAN;
        }
        if decision.eq_ignore_ascii_case(HAS_ISSUES) {
            return HAS_ISSUES;
        }
    }
    HAS_ISSUES
}

pub(crate) fn agreement_level(consensus_result: &ConsensusResult) -> f32 {
    let active: Vec<&AgentResponse> = consensus_result
        .responses
        .iter()
        .filter(|r| !r.timed_out)
        .collect();
    if active.is_empty() {
        return 0.0;
    }

    if let Some(decision) = consensus_result.decision.as_deref() {
        let agreement = active
            .iter()
            .filter(|response| response.content == decision)
            .count();
        return agreement as f32 / active.len() as f32;
    }

    let mut counts: HashMap<&str, usize> = HashMap::new();
    for response in &active {
        *counts.entry(response.content.as_str()).or_insert(0) += 1;
    }
    let max_count = counts.values().copied().max().unwrap_or(0);
    max_count as f32 / active.len() as f32
}

pub(crate) fn consensus_strategy_label(strategy: ConsensusStrategy) -> &'static str {
    match strategy {
        ConsensusStrategy::Majority => "majority",
        ConsensusStrategy::Weighted => "weighted",
        ConsensusStrategy::Unanimous => "unanimous",
        ConsensusStrategy::HumanInTheLoop => "human-in-the-loop",
    }
}

pub(crate) fn merge_related_findings(findings: Vec<Finding>) -> Vec<Finding> {
    let mut merged: Vec<Finding> = Vec::new();

    for finding in findings {
        if let Some(index) = merged
            .iter()
            .position(|existing| are_related_findings(existing, &finding))
        {
            if finding.severity > merged[index].severity {
                merged[index] = finding;
            }
        } else {
            merged.push(finding);
        }
    }

    merged
}

fn are_related_findings(left: &Finding, right: &Finding) -> bool {
    if left.rule_id != right.rule_id || left.file != right.file {
        return false;
    }

    let (Some(left_line), Some(right_line)) = (left.line, right.line) else {
        return false;
    };

    left_line.abs_diff(right_line) <= 2
}

/// Consolidates findings in two steps:
/// 1. Deduplicate by `fid`, retaining the highest-severity entry per ID.
/// 2. Merge related findings (same rule, same file, both with lines within 2 lines),
///    retaining the highest-severity entry per related group.
pub(crate) fn consolidate_findings(findings: Vec<Finding>) -> Vec<Finding> {
    let mut deduped: HashMap<String, Finding> = HashMap::new();

    for finding in findings {
        match deduped.entry(finding.fid.clone()) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(finding);
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                if finding.severity > entry.get().severity {
                    entry.insert(finding);
                }
            }
        }
    }

    // Sort deduped values before relatedness merge to ensure deterministic output
    // regardless of HashMap iteration order.
    let mut deduped_sorted: Vec<Finding> = deduped.into_values().collect();
    deduped_sorted.sort_by(|a, b| a.fid.cmp(&b.fid));
    let mut consolidated = merge_related_findings(deduped_sorted);
    consolidated.sort_by(|left, right| {
        right
            .severity
            .cmp(&left.severity)
            .then_with(|| left.fid.cmp(&right.fid))
    });
    consolidated
}

pub(crate) fn build_consolidated_artifact(
    reviewer_artifacts: Vec<ReviewArtifact>,
    session_id: &str,
) -> ReviewArtifact {
    let review_mode = reviewer_artifacts
        .iter()
        .find_map(|artifact| artifact.review_mode.clone());
    let all_findings: Vec<Finding> = reviewer_artifacts
        .into_iter()
        .flat_map(|artifact| artifact.findings)
        .collect();
    let findings = consolidate_findings(all_findings);
    let severity_summary = SeveritySummary::from_findings(&findings);

    ReviewArtifact {
        findings,
        severity_summary,
        review_mode,
        schema_version: "1.0".to_string(),
        session_id: session_id.to_string(),
        timestamp: chrono::Utc::now(),
    }
}

pub(crate) fn write_consolidated_artifact(
    artifact: &ReviewArtifact,
    output_dir: &Path,
) -> Result<()> {
    let artifact_path = output_dir.join("review-consolidated.json");
    let payload = serde_json::to_string_pretty(artifact)
        .context("failed to serialize consolidated review artifact")?;
    fs::write(&artifact_path, payload).with_context(|| {
        format!(
            "failed to write consolidated review artifact at {}",
            artifact_path.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
#[path = "review_consensus_tests.rs"]
mod tests;
