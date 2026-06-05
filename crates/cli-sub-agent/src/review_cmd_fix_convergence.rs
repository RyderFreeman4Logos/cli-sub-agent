//! Fix-loop terminal convergence decision logic for `csa review --fix`.
//!
//! Split out of `review_cmd_fix.rs` via the `#[path]` sibling idiom to keep
//! each module under the per-module token budget. Pure decision logic over the
//! `(quality_gate_passed, fix_output_was_substantive, final_decision)` triple;
//! performs no IO and owns no session state.

use csa_core::types::ReviewDecision;
use csa_session::FixConvergenceMeta;

pub(super) fn fix_exit_code_for_convergence(
    quality_gate_passed: bool,
    fix_output_was_substantive: bool,
    final_decision: ReviewDecision,
) -> i32 {
    if reached_genuine_clean_convergence(
        quality_gate_passed,
        fix_output_was_substantive,
        final_decision,
    ) {
        0
    } else {
        1
    }
}

pub(super) fn reached_genuine_clean_convergence(
    quality_gate_passed: bool,
    fix_output_was_substantive: bool,
    final_decision: ReviewDecision,
) -> bool {
    quality_gate_passed && fix_output_was_substantive && final_decision == ReviewDecision::Pass
}

pub(super) struct FixTerminalOutcome {
    quality_gate_passed: bool,
    fix_output_was_substantive: bool,
    pub(super) post_consistency_decision: ReviewDecision,
    pub(super) terminal_reason: &'static str,
}

impl FixTerminalOutcome {
    pub(super) fn new(
        quality_gate_passed: bool,
        fix_output_was_substantive: bool,
        post_consistency_decision: ReviewDecision,
    ) -> Self {
        Self {
            quality_gate_passed,
            fix_output_was_substantive,
            post_consistency_decision,
            terminal_reason: terminal_reason_for_convergence(
                quality_gate_passed,
                fix_output_was_substantive,
                post_consistency_decision,
            ),
        }
    }

    pub(super) fn reached_genuine_clean_convergence(&self) -> bool {
        reached_genuine_clean_convergence(
            self.quality_gate_passed,
            self.fix_output_was_substantive,
            self.post_consistency_decision,
        )
    }

    pub(super) fn exit_code(&self) -> i32 {
        fix_exit_code_for_convergence(
            self.quality_gate_passed,
            self.fix_output_was_substantive,
            self.post_consistency_decision,
        )
    }

    pub(super) fn pre_verdict_non_converged(&self) -> bool {
        pre_verdict_non_convergence_reason(
            self.quality_gate_passed,
            self.fix_output_was_substantive,
        )
        .is_some()
    }

    pub(super) fn fix_convergence_meta(&self) -> FixConvergenceMeta {
        FixConvergenceMeta {
            quality_gate_passed: self.quality_gate_passed,
            fix_output_was_substantive: self.fix_output_was_substantive,
            post_consistency_decision: self.post_consistency_decision.as_str().to_string(),
            reached_genuine_clean_convergence: self.reached_genuine_clean_convergence(),
            terminal_reason: self.terminal_reason.to_string(),
        }
    }
}

fn terminal_reason_for_convergence(
    quality_gate_passed: bool,
    fix_output_was_substantive: bool,
    post_consistency_decision: ReviewDecision,
) -> &'static str {
    if reached_genuine_clean_convergence(
        quality_gate_passed,
        fix_output_was_substantive,
        post_consistency_decision,
    ) {
        "clean_convergence"
    } else if !fix_output_was_substantive {
        "empty_fix_output"
    } else if !quality_gate_passed {
        "quality_gate_failed"
    } else {
        "post_consistency_non_pass"
    }
}

pub(super) fn pre_verdict_non_convergence_reason(
    quality_gate_passed: bool,
    fix_output_was_substantive: bool,
) -> Option<&'static str> {
    if !fix_output_was_substantive {
        Some("empty_fix_output")
    } else if !quality_gate_passed {
        Some("quality_gate_failed")
    } else {
        None
    }
}
