use csa_core::types::ReviewDecision;

pub(crate) const INFRASTRUCTURE_FAILURE_EXIT_CODE: i32 = 2;

pub(crate) fn exit_code_from_review_decision(decision: ReviewDecision) -> i32 {
    match decision {
        ReviewDecision::Pass => 0,
        ReviewDecision::Fail
        | ReviewDecision::Skip
        | ReviewDecision::Uncertain
        | ReviewDecision::Unavailable => 1,
    }
}

pub(crate) fn exit_code_from_debate_verdict(verdict: &str, decision: Option<&str>) -> i32 {
    if token_is_success(decision) || token_is_success(Some(verdict)) {
        return 0;
    }

    if token_is_failure(decision) || token_is_failure(Some(verdict)) {
        return 1;
    }

    INFRASTRUCTURE_FAILURE_EXIT_CODE
}

fn token_is_success(token: Option<&str>) -> bool {
    token.is_some_and(|token| {
        matches!(
            token.trim().to_ascii_uppercase().as_str(),
            "APPROVE" | "PASS" | "CLEAN" | "CONFIRMED"
        )
    })
}

fn token_is_failure(token: Option<&str>) -> bool {
    token.is_some_and(|token| {
        matches!(
            token.trim().to_ascii_uppercase().as_str(),
            "REVISE" | "REJECT" | "FAIL" | "HAS_ISSUES" | "UNAVAILABLE"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_pass_maps_to_zero() {
        assert_eq!(exit_code_from_review_decision(ReviewDecision::Pass), 0);
    }

    #[test]
    fn review_fail_maps_to_one() {
        assert_eq!(exit_code_from_review_decision(ReviewDecision::Fail), 1);
    }

    #[test]
    fn debate_approve_maps_to_zero() {
        assert_eq!(exit_code_from_debate_verdict("APPROVE", None), 0);
    }

    #[test]
    fn debate_revise_maps_to_one() {
        assert_eq!(exit_code_from_debate_verdict("REVISE", None), 1);
    }

    #[test]
    fn debate_confirmed_maps_to_zero() {
        // `CSA_VERDICT: CONFIRMED` normalizes to a `CONFIRMED` verdict token.
        assert_eq!(exit_code_from_debate_verdict("CONFIRMED", None), 0);
        assert_eq!(exit_code_from_debate_verdict("MAYBE", Some("confirmed")), 0);
    }

    #[test]
    fn debate_unknown_maps_to_infrastructure_failure() {
        assert_eq!(
            exit_code_from_debate_verdict("MAYBE", None),
            INFRASTRUCTURE_FAILURE_EXIT_CODE
        );
    }
}
