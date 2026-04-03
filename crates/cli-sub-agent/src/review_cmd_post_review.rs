use csa_core::types::ReviewDecision;

const POST_REVIEW_PR_BOT_CMD: &str = "csa plan run --sa-mode true --pattern pr-bot";

pub(super) fn emit_post_review_output(output: &str) {
    let trimmed = output.trim_end();
    if trimmed.is_empty() {
        return;
    }

    // Daemon-mode callers typically observe completion via `csa session wait`,
    // which streams stdout.log only. Mirror the directive there so the normal
    // daemon path can consume it mechanically without tailing stderr.log.
    if std::env::var_os("CSA_DAEMON_SESSION_ID").is_some() {
        println!("{trimmed}");
    }
    eprintln!("{trimmed}");
}

pub(super) fn build_post_review_output(
    captured_output: &str,
    decision: ReviewDecision,
    scope: &str,
) -> String {
    let trimmed = captured_output.trim_end();
    if csa_hooks::parse_next_step_directive(trimmed).is_some() {
        return trimmed.to_string();
    }

    let Some(directive) = synthesize_post_review_next_step(decision, scope) else {
        return trimmed.to_string();
    };

    if trimmed.is_empty() {
        directive
    } else {
        format!("{trimmed}\n{directive}")
    }
}

fn synthesize_post_review_next_step(decision: ReviewDecision, scope: &str) -> Option<String> {
    if decision == ReviewDecision::Pass && review_scope_is_cumulative(scope) {
        return Some(csa_hooks::format_next_step_directive(
            POST_REVIEW_PR_BOT_CMD,
            true,
        ));
    }
    None
}

fn review_scope_is_cumulative(scope: &str) -> bool {
    scope.starts_with("base:") || scope.starts_with("range:")
}
