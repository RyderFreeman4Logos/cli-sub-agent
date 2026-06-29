use std::fs;
use std::path::Path;

use csa_core::types::{ReviewDecision, ToolName};
use csa_session::state::ReviewSessionMeta;
use serde::Serialize;
use tracing::{debug, warn};

const POST_REVIEW_PR_BOT_CMD: &str = "csa plan run --sa-mode true --pattern pr-bot";
pub(super) const CONFIRM_THEN_FIX_FINDING_ACTION: &str = "confirm_then_fix_finding";
const CONFIRM_THEN_FIX_FINDING_HINT: &str = "Confirm the review finding is not a false positive, then run --fix-finding. The fix pass resumes the reviewer session for KV-cache reuse; use a NEW review session for the next review round.";

#[derive(Serialize)]
struct ReviewSuggestionFile {
    suggestion: ReviewFailureSuggestion,
}

#[derive(Serialize)]
struct ReviewFailureSuggestion {
    action: &'static str,
    session_id: String,
    requires_confirmation: bool,
    command_template: String,
    hint: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_spec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_session_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ReviewFailureFixFindingRoute {
    pub(super) tool: Option<String>,
    pub(super) model_spec: Option<String>,
    pub(super) model: Option<String>,
    pub(super) thinking: Option<String>,
    pub(super) provider_session_id: Option<String>,
}

impl ReviewFailureFixFindingRoute {
    fn can_resume_exact_fix_finding_route(&self) -> bool {
        non_empty(self.tool.as_deref())
            && non_empty(self.provider_session_id.as_deref())
            && self.has_exact_model_route()
    }

    fn has_exact_model_route(&self) -> bool {
        non_empty(self.model_spec.as_deref())
            || (non_empty(self.model.as_deref()) && non_empty(self.thinking.as_deref()))
    }

    fn unavailable_reason(&self) -> Option<&'static str> {
        if !non_empty(self.tool.as_deref()) {
            return Some("the failed review did not record the exact review tool");
        }
        if !non_empty(self.provider_session_id.as_deref()) {
            return Some("the backend did not record a provider session id for KV-cache reuse");
        }
        if !self.has_exact_model_route() {
            return Some("the failed review did not record an exact model route");
        }
        None
    }
}

fn non_empty(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.trim().is_empty())
}

pub(super) fn build_fix_finding_route(
    result: &super::execute::ReviewExecutionOutcome,
    initial_tool: ToolName,
    resolved_model_spec: Option<&str>,
    review_model: Option<&str>,
    review_thinking: Option<&str>,
) -> ReviewFailureFixFindingRoute {
    ReviewFailureFixFindingRoute {
        tool: Some(result.executed_tool.to_string()),
        model_spec: result.routed_to.clone().or_else(|| {
            (result.executed_tool == initial_tool)
                .then(|| resolved_model_spec.map(str::to_string))
                .flatten()
        }),
        model: review_model.map(str::to_string),
        thinking: review_thinking.map(str::to_string),
        provider_session_id: result.execution.provider_session_id.clone(),
    }
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
    route: Option<&ReviewFailureFixFindingRoute>,
) -> Option<String> {
    if decision != ReviewDecision::Fail {
        return None;
    }
    let route = route?;
    if !route.can_resume_exact_fix_finding_route() {
        return None;
    }

    let command_template =
        format!("csa review --fix-finding --session {session_id} --prompt-file FIX_PROMPT.md");
    let suggestion = ReviewSuggestionFile {
        suggestion: ReviewFailureSuggestion {
            action: CONFIRM_THEN_FIX_FINDING_ACTION,
            session_id: session_id.to_string(),
            requires_confirmation: true,
            command_template,
            hint: CONFIRM_THEN_FIX_FINDING_HINT,
            tool: route.tool.clone(),
            model_spec: route.model_spec.clone(),
            model: route.model.clone(),
            thinking: route.thinking.clone(),
            provider_session_id: route.provider_session_id.clone(),
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
    route: Option<&ReviewFailureFixFindingRoute>,
) {
    let Some(suggestion) = build_review_failure_suggestion(decision, session_id, route) else {
        return;
    };
    if !preceding_output.is_empty() && !preceding_output.ends_with('\n') {
        println!();
    }
    print!("{suggestion}");
    println!("{}", build_review_failure_caller_hint(session_id));
}

pub(super) fn build_review_failure_fix_finding_unavailable_explanation(
    decision: ReviewDecision,
    session_id: &str,
    route: Option<&ReviewFailureFixFindingRoute>,
) -> Option<String> {
    if decision != ReviewDecision::Fail {
        return None;
    }
    let reason = route
        .and_then(ReviewFailureFixFindingRoute::unavailable_reason)
        .unwrap_or("the failed review did not record exact route metadata");
    Some(format!(
        "No `csa review --fix-finding` suggestion emitted for review session {session_id}: \
         {reason}. Rerun review with an exact `--model-spec` or explicit `--model` and \
         `--thinking`, and ensure the backend records a provider session id; otherwise use \
         `csa review --session {session_id} --fix` for the legacy fix path."
    ))
}

pub(super) fn persist_review_failure_suggestion(
    project_root: &Path,
    meta: &ReviewSessionMeta,
    route: Option<&ReviewFailureFixFindingRoute>,
) {
    let decision = meta
        .decision
        .parse::<ReviewDecision>()
        .unwrap_or(ReviewDecision::Uncertain);
    let suggestion = build_review_failure_suggestion(decision, &meta.session_id, route);

    match csa_session::get_session_dir(project_root, &meta.session_id) {
        Ok(session_dir) => {
            let suggestion_path = session_dir.join("output").join("suggestion.toml");
            let Some(suggestion) = suggestion else {
                if decision == ReviewDecision::Fail
                    && let Err(error) = fs::remove_file(&suggestion_path)
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    warn!(session_id = %meta.session_id, error = %error, "Failed to remove stale output/suggestion.toml");
                }
                return;
            };
            let output_dir = session_dir.join("output");
            if let Err(error) = fs::create_dir_all(&output_dir) {
                warn!(session_id = %meta.session_id, error = %error, "Failed to create review output dir");
                return;
            }
            if let Err(error) = fs::write(suggestion_path, suggestion) {
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
    route: Option<&ReviewFailureFixFindingRoute>,
) {
    let decision = meta
        .decision
        .parse::<ReviewDecision>()
        .unwrap_or(ReviewDecision::Uncertain);
    emit_review_failure_suggestion(decision, &meta.session_id, preceding_output, route);
    if build_review_failure_suggestion(decision, &meta.session_id, route).is_none()
        && let Some(explanation) = build_review_failure_fix_finding_unavailable_explanation(
            decision,
            &meta.session_id,
            route,
        )
    {
        if !preceding_output.is_empty() && !preceding_output.ends_with('\n') {
            println!();
        }
        println!("{explanation}");
    }
    persist_review_failure_suggestion(project_root, meta, route);
}

pub(super) fn build_review_failure_caller_hint(session_id: &str) -> String {
    let command =
        format!("csa review --fix-finding --session {session_id} --prompt-file FIX_PROMPT.md");
    let escaped_command = crate::daemon_caller_hints::escape_structured_comment_attr(&command);
    format!(
        "<!-- CSA:CALLER_HINT action=\"review_confirm_fix_finding\" \
         session_id=\"{session_id}\" requires_confirmation=\"true\" \
         command=\"{escaped_command}\" \
         next_review=\"fresh_session\" -->"
    )
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

    fn exact_route() -> ReviewFailureFixFindingRoute {
        ReviewFailureFixFindingRoute {
            tool: Some("codex".to_string()),
            model_spec: Some("codex/openai/gpt-5.5/xhigh".to_string()),
            provider_session_id: Some("provider-123".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn review_failure_suggestion_renders_confirm_then_fix_finding_block() {
        let route = exact_route();
        let rendered = build_review_failure_suggestion(
            ReviewDecision::Fail,
            "01HREVIEWSESSION0000000000",
            Some(&route),
        )
        .expect("fail decision should render suggestion");

        assert_eq!(
            rendered,
            "[suggestion]\naction = \"confirm_then_fix_finding\"\nsession_id = \"01HREVIEWSESSION0000000000\"\nrequires_confirmation = true\ncommand_template = \"csa review --fix-finding --session 01HREVIEWSESSION0000000000 --prompt-file FIX_PROMPT.md\"\nhint = \"Confirm the review finding is not a false positive, then run --fix-finding. The fix pass resumes the reviewer session for KV-cache reuse; use a NEW review session for the next review round.\"\ntool = \"codex\"\nmodel_spec = \"codex/openai/gpt-5.5/xhigh\"\nprovider_session_id = \"provider-123\"\n"
        );
    }

    #[test]
    fn review_failure_suggestion_renders_exact_route_when_available() {
        let route = exact_route();
        let rendered = build_review_failure_suggestion(
            ReviewDecision::Fail,
            "01HREVIEWSESSION0000000000",
            Some(&route),
        )
        .expect("fail decision should render suggestion");

        assert!(rendered.contains("tool = \"codex\""));
        assert!(rendered.contains("model_spec = \"codex/openai/gpt-5.5/xhigh\""));
        assert!(rendered.contains("provider_session_id = \"provider-123\""));
    }

    #[test]
    fn review_failure_suggestion_requires_usable_exact_route() {
        assert!(
            build_review_failure_suggestion(
                ReviewDecision::Fail,
                "01HREVIEWSESSION0000000000",
                None
            )
            .is_none()
        );

        let missing_model = ReviewFailureFixFindingRoute {
            tool: Some("codex".to_string()),
            provider_session_id: Some("provider-123".to_string()),
            ..Default::default()
        };
        assert!(
            build_review_failure_suggestion(
                ReviewDecision::Fail,
                "01HREVIEWSESSION0000000000",
                Some(&missing_model),
            )
            .is_none()
        );

        let missing_provider = ReviewFailureFixFindingRoute {
            tool: Some("codex".to_string()),
            model_spec: Some("codex/openai/gpt-5.5/xhigh".to_string()),
            ..Default::default()
        };
        assert!(
            build_review_failure_suggestion(
                ReviewDecision::Fail,
                "01HREVIEWSESSION0000000000",
                Some(&missing_provider),
            )
            .is_none()
        );
    }

    #[test]
    fn review_failure_unavailable_explanation_is_actionable() {
        let explanation = build_review_failure_fix_finding_unavailable_explanation(
            ReviewDecision::Fail,
            "01HREVIEWSESSION0000000000",
            None,
        )
        .expect("fail decision without route should explain unavailable fix-finding");

        assert!(explanation.contains("No `csa review --fix-finding` suggestion emitted"));
        assert!(explanation.contains("--model-spec"));
        assert!(explanation.contains("--session 01HREVIEWSESSION0000000000 --fix"));
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
                build_review_failure_suggestion(decision, "01HREVIEWSESSION0000000000", None)
                    .is_none(),
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
            review_mode: None,
            fix_convergence: None,
        };

        let route = exact_route();

        persist_review_failure_suggestion(&project_root, &meta, Some(&route));

        let suggestion_path = csa_session::get_session_dir(&project_root, &session.meta_session_id)
            .expect("session dir")
            .join("output")
            .join("suggestion.toml");
        let rendered = std::fs::read_to_string(suggestion_path).expect("suggestion.toml");
        assert_eq!(
            rendered,
            build_review_failure_suggestion(
                ReviewDecision::Fail,
                &session.meta_session_id,
                Some(&route),
            )
            .expect("expected rendered suggestion")
        );
    }

    #[test]
    fn review_failure_suggestion_removes_stale_sidecar_when_route_unusable() {
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
            review_mode: None,
            fix_convergence: None,
        };
        let suggestion_path = csa_session::get_session_dir(&project_root, &session.meta_session_id)
            .expect("session dir")
            .join("output")
            .join("suggestion.toml");
        std::fs::create_dir_all(suggestion_path.parent().expect("output dir")).expect("output dir");
        std::fs::write(&suggestion_path, "stale").expect("stale suggestion");

        persist_review_failure_suggestion(&project_root, &meta, None);

        assert!(!suggestion_path.exists());
    }
}
