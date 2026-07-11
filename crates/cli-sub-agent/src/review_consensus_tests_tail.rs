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
fn multi_reviewer_majority_uncertain_preserves_uncertain_verdict() {
    let responses = vec![
        response("reviewer-1:codex", UNCERTAIN, false),
        response("reviewer-2:opencode", UNCERTAIN, false),
        response("reviewer-3:claude-code", CLEAN, false),
    ];

    let consensus = resolve_consensus(ConsensusStrategy::Majority, &responses);
    let final_verdict = consensus_verdict(&consensus);

    assert!(consensus.consensus_reached);
    assert_eq!(consensus.decision.as_deref(), Some(UNCERTAIN));
    assert_eq!(final_verdict, UNCERTAIN);
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

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn majority_consensus_excludes_unavailable_reviewers_before_voting(
        states in prop::collection::vec(reviewer_state_strategy(), 2..=4),
    ) {
        let responses: Vec<AgentResponse> = states
            .iter()
            .enumerate()
            .map(|(idx, state)| {
                response(
                    &format!("reviewer-{idx}"),
                    state.verdict(),
                    matches!(state, ReviewerState::Unavailable),
                )
            })
            .collect();
        let active_responses: Vec<AgentResponse> = responses
            .iter()
            .filter(|response| !response.timed_out)
            .cloned()
            .collect();

        let consensus_with_unavailable =
            resolve_consensus(ConsensusStrategy::Majority, &responses);
        let consensus_without_unavailable =
            resolve_consensus(ConsensusStrategy::Majority, &active_responses);

        prop_assert_eq!(
            consensus_with_unavailable.decision.as_deref(),
            consensus_without_unavailable.decision.as_deref()
        );
        prop_assert_eq!(
            consensus_verdict(&consensus_with_unavailable),
            consensus_verdict(&consensus_without_unavailable)
        );
        prop_assert!(
            (agreement_level(&consensus_with_unavailable)
                - agreement_level(&consensus_without_unavailable))
                .abs()
                < f32::EPSILON
        );
    }
}

fn reviewer_state_strategy() -> impl Strategy<Value = ReviewerState> {
    prop_oneof![
        Just(ReviewerState::Pass),
        Just(ReviewerState::Fail),
        Just(ReviewerState::Unavailable),
    ]
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
        finding("FID-1", Severity::Low),
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
    ]);

    let severities: Vec<Severity> = consolidated.into_iter().map(|item| item.severity).collect();
    assert_eq!(
        severities,
        vec![
            Severity::Critical,
            Severity::High,
            Severity::Medium,
            Severity::Low,
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

    let artifact_path = temp.path().join(CONSOLIDATED_REVIEW_ARTIFACT_FILE);
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

    assert_eq!(
        parse_review_decision("PASS but also HAS_ISSUES", 0),
        ReviewDecision::Fail
    );
}

#[test]
fn parse_review_decision_exit_code_fallback() {
    use csa_core::types::ReviewDecision;

    assert_eq!(
        parse_review_decision("no verdict here", 0),
        ReviewDecision::Fail
    );
    assert_eq!(
        parse_review_decision("no verdict here", 1),
        ReviewDecision::Fail
    );
}

#[test]
fn parse_review_decision_does_not_treat_findings_as_pass_from_exit_zero() {
    use csa_core::types::ReviewDecision;

    assert_eq!(
        parse_review_decision("1. P1 issue in cli.rs", 0),
        ReviewDecision::Fail
    );
}

#[test]
fn parse_review_decision_accepts_clean_phrase_without_explicit_token() {
    use csa_core::types::ReviewDecision;

    assert_eq!(
        parse_review_decision("No blocking issues found.", 0),
        ReviewDecision::Pass
    );
    assert_eq!(
        parse_review_decision("No security, privacy, or safety issues were introduced.", 0),
        ReviewDecision::Pass
    );
}
