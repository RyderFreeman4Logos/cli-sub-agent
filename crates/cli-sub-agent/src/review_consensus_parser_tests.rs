use super::*;

#[test]
fn parse_review_decision_accepts_clean_pass_section() {
    use csa_core::types::ReviewDecision;

    assert_eq!(
        parse_review_decision(
            "<!-- CSA:SECTION:summary -->\nPASS\n<!-- CSA:SECTION:summary:END -->\n\
             <!-- CSA:SECTION:details -->\nNo blocking correctness issues found.\n<!-- CSA:SECTION:details:END -->",
            0,
        ),
        ReviewDecision::Pass
    );
}

#[test]
fn parse_review_decision_accepts_markdown_pass_verdict() {
    use csa_core::types::ReviewDecision;

    assert_eq!(
        parse_review_decision("## Verdict\n\n**PASS**\n\nNo findings.", 0),
        ReviewDecision::Pass
    );
}

#[test]
fn parse_review_decision_fails_for_high_findings() {
    use csa_core::types::ReviewDecision;

    assert_eq!(
        parse_review_decision(
            "Verdict: FAIL\n\nFindings\n1. [High][correctness] incorrect cache key.",
            1,
        ),
        ReviewDecision::Fail
    );
}

#[test]
fn parse_review_decision_no_issues_found_is_not_has_issues() {
    use csa_core::types::ReviewDecision;

    assert_eq!(
        parse_review_decision("No issues found.", 0),
        ReviewDecision::Pass
    );
}
