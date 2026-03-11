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
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
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
        review_mode: None,
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
    let tools = build_reviewer_tools(Some(ToolName::Codex), ToolName::Codex, Some(&cfg), None, 3);
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
        finding_with_location(
            "FID-B",
            Severity::High,
            "src/main.rs",
            "rule.same",
            Some(13),
        ),
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
        finding_with_location(
            "FID-B",
            Severity::Critical,
            "src/main.rs",
            "rule.same",
            Some(10),
        ),
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
    let source = finding_with_location(
        "FID-ONLY",
        Severity::Medium,
        "src/lib.rs",
        "rule.one",
        Some(1),
    );
    let merged = merge_related_findings(vec![source.clone()]);

    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0], source);
}

#[test]
fn build_consolidated_artifact_merges_findings_from_two_reviewers() {
    let reviewer_one = artifact_with_findings(
        "session-a",
        vec![
            finding("FID-SHARED", Severity::Low),
            finding("FID-A", Severity::High),
        ],
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
fn build_consolidated_artifact_preserves_first_review_mode() {
    let reviewer_one = ReviewArtifact {
        review_mode: Some("range:main...HEAD".to_string()),
        ..artifact_with_findings("session-a", vec![finding("FID-A", Severity::High)])
    };
    let reviewer_two = ReviewArtifact {
        review_mode: Some("diff".to_string()),
        ..artifact_with_findings("session-b", vec![finding("FID-B", Severity::Low)])
    };

    let consolidated =
        build_consolidated_artifact(vec![reviewer_one, reviewer_two], "session-final");

    assert_eq!(
        consolidated.review_mode.as_deref(),
        Some("range:main...HEAD")
    );
}

#[test]
fn build_consolidated_artifact_with_empty_input_produces_empty_artifact() {
    let consolidated = build_consolidated_artifact(Vec::new(), "session-empty");

    assert_eq!(consolidated.session_id, "session-empty");
    assert_eq!(consolidated.schema_version, "1.0");
    assert_eq!(consolidated.review_mode, None);
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

#[test]
fn parse_review_decision_four_values() {
    use csa_core::types::ReviewDecision;

    assert_eq!(
        parse_review_decision("Verdict: PASS", 0),
        ReviewDecision::Pass
    );
    assert_eq!(
        parse_review_decision("Verdict: CLEAN", 0),
        ReviewDecision::Pass
    );
    assert_eq!(
        parse_review_decision("Verdict: FAIL", 1),
        ReviewDecision::Fail
    );
    assert_eq!(
        parse_review_decision("Verdict: HAS_ISSUES", 1),
        ReviewDecision::Fail
    );
    assert_eq!(
        parse_review_decision("Verdict: SKIP", 0),
        ReviewDecision::Skip
    );
    assert_eq!(
        parse_review_decision("Verdict: UNCERTAIN", 0),
        ReviewDecision::Uncertain
    );
}

#[test]
fn parse_review_decision_fail_takes_priority() {
    use csa_core::types::ReviewDecision;

    // When both FAIL and PASS tokens present, FAIL wins
    assert_eq!(
        parse_review_decision("PASS but also HAS_ISSUES", 0),
        ReviewDecision::Fail
    );
}

#[test]
fn parse_review_decision_exit_code_fallback() {
    use csa_core::types::ReviewDecision;

    // No verdict token: fall back to exit code
    assert_eq!(
        parse_review_decision("no verdict here", 0),
        ReviewDecision::Pass
    );
    assert_eq!(
        parse_review_decision("no verdict here", 1),
        ReviewDecision::Fail
    );
}
