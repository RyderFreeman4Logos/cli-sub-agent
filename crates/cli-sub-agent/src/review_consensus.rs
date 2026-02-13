use anyhow::Result;
use std::collections::HashMap;

use csa_config::ProjectConfig;
use csa_core::consensus::{
    AgentResponse, ConsensusResult, ConsensusStrategy, resolve_majority, resolve_unanimous,
    resolve_weighted,
};
use csa_core::types::ToolName;

pub(crate) const CLEAN: &str = "CLEAN";
pub(crate) const HAS_ISSUES: &str = "HAS_ISSUES";

pub(crate) fn build_reviewer_tools(
    explicit_tool: Option<ToolName>,
    primary_tool: ToolName,
    project_config: Option<&ProjectConfig>,
    reviewer_count: usize,
) -> Vec<ToolName> {
    if reviewer_count == 0 {
        return Vec::new();
    }
    if explicit_tool.is_some() {
        return vec![primary_tool; reviewer_count];
    }

    let enabled_tools: Vec<ToolName> = if let Some(cfg) = project_config {
        csa_config::global::all_known_tools()
            .iter()
            .filter(|t| cfg.is_tool_enabled(t.as_str()))
            .copied()
            .collect()
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
    let output_dir = format!(".csa/reviewers/reviewer-{reviewer_index}");
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

#[cfg(test)]
mod tests {
    use super::*;
    use csa_config::{ProjectMeta, ResourcesConfig, ToolConfig};

    fn project_config_with_enabled_tools(tools: &[&str]) -> ProjectConfig {
        let mut tool_map = HashMap::new();
        for tool in csa_config::global::all_known_tools() {
            tool_map.insert(
                tool.as_str().to_string(),
                ToolConfig {
                    enabled: false,
                    restrictions: None,
                    suppress_notify: false,
                },
            );
        }
        for tool in tools {
            tool_map.insert(
                (*tool).to_string(),
                ToolConfig {
                    enabled: true,
                    restrictions: None,
                    suppress_notify: false,
                },
            );
        }

        ProjectConfig {
            schema_version: 1,
            project: ProjectMeta::default(),
            resources: ResourcesConfig::default(),
            tools: tool_map,
            review: None,
            debate: None,
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
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

    #[test]
    fn build_reviewer_tools_returns_empty_when_reviewer_count_is_zero() {
        let cfg = project_config_with_enabled_tools(&["codex", "opencode"]);
        let tools = build_reviewer_tools(None, ToolName::Codex, Some(&cfg), 0);
        assert!(tools.is_empty());
    }

    #[test]
    fn build_reviewer_tools_round_robin_across_enabled_tools() {
        let cfg = project_config_with_enabled_tools(&["codex", "claude-code", "opencode"]);
        let tools = build_reviewer_tools(None, ToolName::Codex, Some(&cfg), 5);
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
        let tools = build_reviewer_tools(Some(ToolName::Codex), ToolName::Codex, Some(&cfg), 3);
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
}
