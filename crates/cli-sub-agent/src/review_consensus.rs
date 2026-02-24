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
You are reviewer {reviewer_index}. Emit exactly one final verdict token: {CLEAN} or {HAS_ISSUES}.\n\
Write review artifacts to {output_dir}/review-findings.json and {output_dir}/review-report.md.\n\
If no serious issues (P0/P1), verdict must be {CLEAN}; otherwise verdict must be {HAS_ISSUES}.\n\
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
    let all_findings: Vec<Finding> = reviewer_artifacts
        .into_iter()
        .flat_map(|artifact| artifact.findings)
        .collect();
    let findings = consolidate_findings(all_findings);
    let severity_summary = SeveritySummary::from_findings(&findings);

    ReviewArtifact {
        findings,
        severity_summary,
        schema_version: "1.0".to_string(),
        session_id: session_id.to_string(),
        timestamp: chrono::Utc::now(),
    }
}

pub(crate) fn write_consolidated_artifact(artifact: &ReviewArtifact, output_dir: &Path) -> Result<()> {
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
mod tests {
    use super::*;
    use csa_config::{ProjectMeta, ResourcesConfig, ToolConfig};
    use csa_session::review_artifact::{Finding, ReviewArtifact, Severity, SeveritySummary};
    use tempfile::tempdir;

    fn project_config_with_enabled_tools(tools: &[&str]) -> ProjectConfig {
        let mut tool_map = HashMap::new();
        for tool in csa_config::global::all_known_tools() {
            tool_map.insert(
                tool.as_str().to_string(),
                ToolConfig {
                    enabled: false,
                    restrictions: None,
                    suppress_notify: true,
                    ..Default::default()
                },
            );
        }
        for tool in tools {
            tool_map.insert(
                (*tool).to_string(),
                ToolConfig {
                    enabled: true,
                    restrictions: None,
                    suppress_notify: true,
                    ..Default::default()
                },
            );
        }

        ProjectConfig {
            schema_version: 1,
            project: ProjectMeta::default(),
            resources: ResourcesConfig::default(),
            acp: Default::default(),
            tools: tool_map,
            review: None,
            debate: None,
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
            preferences: None,
            session: Default::default(),
        }
    }

    fn response(agent: &str, verdict: &str, timed_out: bool) -> AgentResponse {
        AgentResponse {
            agent: agent.to_string(),
            content: verdict.to_string(),
            weight: 1.0,
            timed_out,
        }
    }

    fn verdict_to_exit_code(verdict: &str) -> i32 {
        if verdict == CLEAN { 0 } else { 1 }
    }

    fn finding_with_location(
        fid: &str,
        severity: Severity,
        file: &str,
        rule_id: &str,
        line: Option<u32>,
    ) -> Finding {
        Finding {
            severity,
            fid: fid.to_string(),
            file: file.to_string(),
            line,
            rule_id: rule_id.to_string(),
            summary: format!("finding-{fid}"),
            engine: "reviewer".to_string(),
        }
    }

    fn finding(fid: &str, severity: Severity) -> Finding {
        finding_with_location(
            fid,
            severity,
            "src/lib.rs",
            &format!("rule.sample.{fid}"),
            Some(1),
        )
    }

    fn artifact_with_findings(session_id: &str, findings: Vec<Finding>) -> ReviewArtifact {
        ReviewArtifact {
            severity_summary: SeveritySummary::from_findings(&findings),
            findings,
            schema_version: "1.0".to_string(),
            session_id: session_id.to_string(),
            timestamp: chrono::Utc::now(),
        }
    }

    #[test]
    fn build_reviewer_tools_returns_empty_when_reviewer_count_is_zero() {
        let cfg = project_config_with_enabled_tools(&["codex", "opencode"]);
        let tools = build_reviewer_tools(None, ToolName::Codex, Some(&cfg), None, 0);
        assert!(tools.is_empty());
    }

    #[test]
    fn build_reviewer_tools_round_robin_across_enabled_tools() {
        let cfg = project_config_with_enabled_tools(&["codex", "claude-code", "opencode"]);
        let tools = build_reviewer_tools(None, ToolName::Codex, Some(&cfg), None, 5);
        assert_eq!(
            tools,
            vec![
                ToolName::Codex,
                ToolName::Opencode,
                ToolName::ClaudeCode,
                ToolName::Codex,
                ToolName::Opencode
            ]
        );
    }

    #[test]
    fn build_reviewer_tools_respects_explicit_tool_override() {
        let cfg = project_config_with_enabled_tools(&["codex", "claude-code", "opencode"]);
        let tools =
            build_reviewer_tools(Some(ToolName::Codex), ToolName::Codex, Some(&cfg), None, 3);
        assert_eq!(
            tools,
            vec![ToolName::Codex, ToolName::Codex, ToolName::Codex]
        );
    }

    #[test]
    fn parse_review_verdict_prefers_has_issues_token() {
        let output = "result: CLEAN but escalation says HAS_ISSUES";
        assert_eq!(parse_review_verdict(output, 0), HAS_ISSUES);
    }

    #[test]
    fn parse_review_verdict_falls_back_to_exit_code() {
        assert_eq!(parse_review_verdict("no explicit verdict", 0), CLEAN);
        assert_eq!(parse_review_verdict("no explicit verdict", 1), HAS_ISSUES);
    }

    #[test]
    fn parse_review_verdict_is_case_insensitive_and_token_aware() {
        assert_eq!(
            parse_review_verdict("final verdict: clean.", 1),
            CLEAN,
            "token matching should be case-insensitive"
        );
        assert_eq!(
            parse_review_verdict("status: unclean output", 1),
            HAS_ISSUES,
            "partial-word matches must not be treated as CLEAN"
        );
    }

    #[test]
    fn parse_consensus_strategy_supports_all_cli_values() {
        assert_eq!(
            parse_consensus_strategy("majority").unwrap(),
            ConsensusStrategy::Majority
        );
        assert_eq!(
            parse_consensus_strategy("weighted").unwrap(),
            ConsensusStrategy::Weighted
        );
        assert_eq!(
            parse_consensus_strategy("unanimous").unwrap(),
            ConsensusStrategy::Unanimous
        );
        assert!(parse_consensus_strategy("invalid").is_err());
    }

    #[test]
    fn agreement_level_uses_top_cluster_when_no_consensus_decision() {
        let responses = vec![
            AgentResponse {
                agent: "r1".to_string(),
                content: CLEAN.to_string(),
                weight: 1.0,
                timed_out: false,
            },
            AgentResponse {
                agent: "r2".to_string(),
                content: HAS_ISSUES.to_string(),
                weight: 1.0,
                timed_out: false,
            },
            AgentResponse {
                agent: "r3".to_string(),
                content: HAS_ISSUES.to_string(),
                weight: 1.0,
                timed_out: false,
            },
        ];
        let result = resolve_unanimous(&responses);
        assert!((agreement_level(&result) - (2.0 / 3.0)).abs() < f32::EPSILON);
        assert_eq!(consensus_verdict(&result), HAS_ISSUES);
    }

    #[test]
    fn multi_reviewer_majority_clean_maps_to_exit_code_zero() {
        let responses = vec![
            response("reviewer-1:codex", parse_review_verdict("CLEAN", 0), false),
            response(
                "reviewer-2:opencode",
                parse_review_verdict("CLEAN", 0),
                false,
            ),
            response(
                "reviewer-3:claude-code",
                parse_review_verdict("HAS_ISSUES", 1),
                false,
            ),
        ];

        let consensus = resolve_consensus(ConsensusStrategy::Majority, &responses);
        let final_verdict = consensus_verdict(&consensus);

        assert!(consensus.consensus_reached);
        assert_eq!(final_verdict, CLEAN);
        assert_eq!(verdict_to_exit_code(final_verdict), 0);
        assert!((agreement_level(&consensus) - (2.0 / 3.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn multi_reviewer_unanimous_disagreement_maps_to_exit_code_one() {
        let responses = vec![
            response("reviewer-1:codex", CLEAN, false),
            response("reviewer-2:opencode", HAS_ISSUES, false),
            response("reviewer-3:claude-code", CLEAN, false),
        ];

        let consensus = resolve_consensus(ConsensusStrategy::Unanimous, &responses);
        let final_verdict = consensus_verdict(&consensus);

        assert!(!consensus.consensus_reached);
        assert!(consensus.decision.is_none());
        assert_eq!(final_verdict, HAS_ISSUES);
        assert_eq!(verdict_to_exit_code(final_verdict), 1);
    }

    #[test]
    fn agreement_level_ignores_timed_out_responses_with_consensus_decision() {
        let responses = vec![
            response("reviewer-1:codex", CLEAN, false),
            response("reviewer-2:opencode", CLEAN, false),
            response("reviewer-3:claude-code", HAS_ISSUES, true),
        ];
        let consensus = resolve_consensus(ConsensusStrategy::Majority, &responses);

        assert_eq!(consensus.decision.as_deref(), Some(CLEAN));
        assert!((agreement_level(&consensus) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn consolidate_findings_deduplicates_by_fid_and_keeps_highest_severity() {
        let consolidated = consolidate_findings(vec![
            finding("DUP-FID", Severity::Low),
            finding("DUP-FID", Severity::Critical),
            finding("UNIQ-FID", Severity::Medium),
        ]);

        assert_eq!(consolidated.len(), 2);
        let duplicate = consolidated
            .iter()
            .find(|item| item.fid == "DUP-FID")
            .expect("deduplicated finding should exist");
        assert_eq!(duplicate.severity, Severity::Critical);
    }

    #[test]
    fn consolidate_findings_with_no_duplicates_preserves_all_findings() {
        let consolidated = consolidate_findings(vec![
            finding("FID-1", Severity::Info),
            finding("FID-2", Severity::Low),
            finding("FID-3", Severity::High),
        ]);

        assert_eq!(consolidated.len(), 3);
        assert!(consolidated.iter().any(|item| item.fid == "FID-1"));
        assert!(consolidated.iter().any(|item| item.fid == "FID-2"));
        assert!(consolidated.iter().any(|item| item.fid == "FID-3"));
    }

    #[test]
    fn consolidate_findings_returns_findings_sorted_by_severity_desc() {
        let consolidated = consolidate_findings(vec![
            finding("FID-LOW", Severity::Low),
            finding("FID-CRIT", Severity::Critical),
            finding("FID-HIGH", Severity::High),
            finding("FID-MED", Severity::Medium),
            finding("FID-INFO", Severity::Info),
        ]);

        let severities: Vec<Severity> = consolidated.into_iter().map(|item| item.severity).collect();
        assert_eq!(
            severities,
            vec![
                Severity::Critical,
                Severity::High,
                Severity::Medium,
                Severity::Low,
                Severity::Info
            ]
        );
    }

    #[test]
    fn merge_related_findings_merges_same_rule_same_file_with_line_delta_two() {
        let merged = merge_related_findings(vec![
            finding_with_location("FID-A", Severity::Low, "src/main.rs", "rule.same", Some(10)),
            finding_with_location(
                "FID-B",
                Severity::Critical,
                "src/main.rs",
                "rule.same",
                Some(12),
            ),
        ]);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].severity, Severity::Critical);
    }

    #[test]
    fn merge_related_findings_keeps_both_when_line_delta_is_three() {
        let merged = merge_related_findings(vec![
            finding_with_location("FID-A", Severity::Low, "src/main.rs", "rule.same", Some(10)),
            finding_with_location("FID-B", Severity::High, "src/main.rs", "rule.same", Some(13)),
        ]);

        assert_eq!(merged.len(), 2);
        assert!(merged.iter().any(|item| item.fid == "FID-A"));
        assert!(merged.iter().any(|item| item.fid == "FID-B"));
    }

    #[test]
    fn merge_related_findings_keeps_both_for_different_files() {
        let merged = merge_related_findings(vec![
            finding_with_location("FID-A", Severity::Low, "src/a.rs", "rule.same", Some(20)),
            finding_with_location("FID-B", Severity::High, "src/b.rs", "rule.same", Some(21)),
        ]);

        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn merge_related_findings_keeps_both_for_different_rules() {
        let merged = merge_related_findings(vec![
            finding_with_location("FID-A", Severity::Low, "src/main.rs", "rule.a", Some(30)),
            finding_with_location("FID-B", Severity::High, "src/main.rs", "rule.b", Some(30)),
        ]);

        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn merge_related_findings_does_not_merge_when_any_line_is_none() {
        let merged = merge_related_findings(vec![
            finding_with_location("FID-A", Severity::Low, "src/main.rs", "rule.same", None),
            finding_with_location("FID-B", Severity::Critical, "src/main.rs", "rule.same", Some(10)),
        ]);

        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn merge_related_findings_returns_empty_for_empty_input() {
        let merged = merge_related_findings(Vec::new());
        assert!(merged.is_empty());
    }

    #[test]
    fn merge_related_findings_returns_single_finding_as_is() {
        let source = finding_with_location("FID-ONLY", Severity::Medium, "src/lib.rs", "rule.one", Some(1));
        let merged = merge_related_findings(vec![source.clone()]);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0], source);
    }

    #[test]
    fn build_consolidated_artifact_merges_findings_from_two_reviewers() {
        let reviewer_one = artifact_with_findings(
            "session-a",
            vec![finding("FID-SHARED", Severity::Low), finding("FID-A", Severity::High)],
        );
        let reviewer_two = artifact_with_findings(
            "session-b",
            vec![
                finding("FID-SHARED", Severity::Critical),
                finding("FID-B", Severity::Medium),
            ],
        );

        let consolidated =
            build_consolidated_artifact(vec![reviewer_one, reviewer_two], "session-final");

        assert_eq!(consolidated.session_id, "session-final");
        assert_eq!(consolidated.schema_version, "1.0");
        assert_eq!(consolidated.findings.len(), 3);
        assert_eq!(consolidated.severity_summary.critical, 1);
        assert_eq!(consolidated.severity_summary.high, 1);
        assert_eq!(consolidated.severity_summary.medium, 1);
        assert_eq!(consolidated.severity_summary.low, 0);
        assert_eq!(consolidated.severity_summary.info, 0);
    }

    #[test]
    fn build_consolidated_artifact_with_empty_input_produces_empty_artifact() {
        let consolidated = build_consolidated_artifact(Vec::new(), "session-empty");

        assert_eq!(consolidated.session_id, "session-empty");
        assert_eq!(consolidated.schema_version, "1.0");
        assert!(consolidated.findings.is_empty());
        assert_eq!(consolidated.severity_summary, SeveritySummary::default());
    }

    #[test]
    fn write_consolidated_artifact_creates_json_file_at_expected_path() {
        let temp = tempdir().expect("tempdir should be created");
        let artifact = artifact_with_findings("session-write", vec![finding("FID-1", Severity::Low)]);

        write_consolidated_artifact(&artifact, temp.path()).expect("artifact should be written");

        let artifact_path = temp.path().join("review-consolidated.json");
        assert!(artifact_path.exists());
        let contents = std::fs::read_to_string(&artifact_path).expect("json file should be readable");
        let parsed: ReviewArtifact =
            serde_json::from_str(&contents).expect("json file should deserialize");
        assert_eq!(parsed.session_id, "session-write");
    }
}
