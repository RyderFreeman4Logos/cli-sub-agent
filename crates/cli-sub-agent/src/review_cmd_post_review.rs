use std::fs;
use std::path::Path;

use csa_core::types::ReviewDecision;
use csa_session::state::ReviewSessionMeta;
use serde::Serialize;
use tracing::{debug, warn};

const POST_REVIEW_PR_BOT_CMD: &str = "csa plan run --sa-mode true --pattern pr-bot";
const RESUME_TO_FIX_ACTION: &str = "resume_to_fix";
const RESUME_TO_FIX_HINT: &str = "Resume this session to fix — reviewer KV cache is warm with full diff context. Use NEW session for next review round.";

#[derive(Serialize)]
struct ReviewSuggestionFile<'a> {
    suggestion: ReviewFailureSuggestion<'a>,
}

#[derive(Serialize)]
struct ReviewFailureSuggestion<'a> {
    action: &'static str,
    session_id: &'a str,
    hint: &'static str,
}

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

pub(super) fn build_review_failure_suggestion(
    decision: ReviewDecision,
    session_id: &str,
) -> Option<String> {
    if decision != ReviewDecision::Fail {
        return None;
    }

    let suggestion = ReviewSuggestionFile {
        suggestion: ReviewFailureSuggestion {
            action: RESUME_TO_FIX_ACTION,
            session_id,
            hint: RESUME_TO_FIX_HINT,
        },
    };
    match toml::to_string_pretty(&suggestion) {
        Ok(rendered) => Some(rendered),
        Err(error) => {
            warn!(session_id, error = %error, "Failed to render review failure suggestion");
            None
        }
    }
}

pub(super) fn emit_review_failure_suggestion(
    decision: ReviewDecision,
    session_id: &str,
    preceding_output: &str,
) {
    let Some(suggestion) = build_review_failure_suggestion(decision, session_id) else {
        return;
    };
    if !preceding_output.is_empty() && !preceding_output.ends_with('\n') {
        println!();
    }
    print!("{suggestion}");
}

pub(super) fn persist_review_failure_suggestion(project_root: &Path, meta: &ReviewSessionMeta) {
    let Some(suggestion) = build_review_failure_suggestion(
        meta.decision
            .parse::<ReviewDecision>()
            .unwrap_or(ReviewDecision::Uncertain),
        &meta.session_id,
    ) else {
        return;
    };

    match csa_session::get_session_dir(project_root, &meta.session_id) {
        Ok(session_dir) => {
            let output_dir = session_dir.join("output");
            if let Err(error) = fs::create_dir_all(&output_dir) {
                warn!(session_id = %meta.session_id, error = %error, "Failed to create review output dir");
                return;
            }
            if let Err(error) = fs::write(output_dir.join("suggestion.toml"), suggestion) {
                warn!(session_id = %meta.session_id, error = %error, "Failed to write output/suggestion.toml");
            } else {
                debug!(session_id = %meta.session_id, "Wrote output/suggestion.toml");
            }
        }
        Err(error) => {
            warn!(session_id = %meta.session_id, error = %error, "Cannot resolve session dir for review suggestion");
        }
    }
}

pub(super) fn suggest_review_failure_fix(
    project_root: &Path,
    meta: &ReviewSessionMeta,
    preceding_output: &str,
) {
    emit_review_failure_suggestion(
        meta.decision
            .parse::<ReviewDecision>()
            .unwrap_or(ReviewDecision::Uncertain),
        &meta.session_id,
        preceding_output,
    );
    persist_review_failure_suggestion(project_root, meta);
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

pub(crate) fn review_scope_is_cumulative(scope: &str) -> bool {
    scope.starts_with("base:") || scope.starts_with("range:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_failure_suggestion_renders_resume_to_fix_block() {
        let rendered =
            build_review_failure_suggestion(ReviewDecision::Fail, "01HREVIEWSESSION0000000000")
                .expect("fail decision should render suggestion");

        assert_eq!(
            rendered,
            "[suggestion]\naction = \"resume_to_fix\"\nsession_id = \"01HREVIEWSESSION0000000000\"\nhint = \"Resume this session to fix — reviewer KV cache is warm with full diff context. Use NEW session for next review round.\"\n"
        );
    }

    #[test]
    fn review_failure_suggestion_only_renders_for_fail_decision() {
        for decision in [
            ReviewDecision::Pass,
            ReviewDecision::Skip,
            ReviewDecision::Uncertain,
            ReviewDecision::Unavailable,
        ] {
            assert!(
                build_review_failure_suggestion(decision, "01HREVIEWSESSION0000000000").is_none(),
                "{decision:?} should not render suggestion"
            );
        }
    }

    #[test]
    fn review_failure_suggestion_persists_to_session_output() {
        let _env_lock = crate::test_env_lock::TEST_ENV_LOCK
            .clone()
            .blocking_lock_owned();
        let temp = tempfile::tempdir().expect("tempdir");
        let project_root = temp.path().join("project");
        let state_home = temp.path().join("state");
        std::fs::create_dir_all(&project_root).expect("project dir");
        std::fs::create_dir_all(&state_home).expect("state dir");
        let _state_home =
            crate::test_env_lock::ScopedEnvVarRestore::set("XDG_STATE_HOME", &state_home);
        let session =
            csa_session::create_session(&project_root, Some("review"), None, Some("codex"))
                .expect("create session");
        let meta = ReviewSessionMeta {
            session_id: session.meta_session_id.clone(),
            head_sha: "HEAD".to_string(),
            decision: ReviewDecision::Fail.as_str().to_string(),
            verdict: "HAS_ISSUES".to_string(),
            status_reason: None,
            routed_to: None,
            primary_failure: None,
            failure_reason: None,
            tool: "codex".to_string(),
            scope: "range:main...HEAD".to_string(),
            exit_code: 1,
            fix_attempted: false,
            fix_rounds: 0,
            review_iterations: 1,
            timestamp: chrono::Utc::now(),
            diff_fingerprint: None,
        };

        persist_review_failure_suggestion(&project_root, &meta);

        let suggestion_path = csa_session::get_session_dir(&project_root, &session.meta_session_id)
            .expect("session dir")
            .join("output")
            .join("suggestion.toml");
        let rendered = std::fs::read_to_string(suggestion_path).expect("suggestion.toml");
        assert_eq!(
            rendered,
            build_review_failure_suggestion(ReviewDecision::Fail, &session.meta_session_id)
                .expect("expected rendered suggestion")
        );
    }
}
