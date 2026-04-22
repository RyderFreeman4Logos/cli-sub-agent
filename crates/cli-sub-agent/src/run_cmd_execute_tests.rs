use super::{finalize_prompt_text, resolve_run_tier_context};
use crate::run_cmd_tool_selection::resolve_skill_and_prompt;
use crate::test_session_sandbox::ScopedSessionSandbox;
use chrono::Utc;
use csa_config::{ProjectConfig, ProjectMeta, TierConfig, TierStrategy};
use std::collections::HashMap;
use std::fs;
use tempfile::TempDir;

fn make_test_config() -> ProjectConfig {
    let mut tiers = HashMap::new();
    tiers.insert(
        "tier-3-complex".to_string(),
        TierConfig {
            description: "test".to_string(),
            models: vec![
                "codex/openai/o4-mini/high".to_string(),
                "claude-code/anthropic/claude-sonnet/high".to_string(),
            ],
            strategy: TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );

    ProjectConfig {
        schema_version: 1,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: Default::default(),
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers,
        tier_mapping: HashMap::from([("default".to_string(), "tier-3-complex".to_string())]),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    }
}

#[test]
fn finalize_prompt_text_prepends_atomic_commit_preamble() {
    let tmp = TempDir::new().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);

    let result = finalize_prompt_text(tmp.path(), "user task".to_string(), None).expect("finalize");

    assert!(
        result.contains("<atomic-commit-discipline>"),
        "preamble must appear in finalized prompt; got: {result}"
    );
    assert!(result.contains("user task"));
    assert!(
        result.find("<atomic-commit-discipline>").unwrap() < result.find("user task").unwrap(),
        "preamble must come BEFORE the user prompt"
    );
}

#[test]
fn finalize_prompt_text_prepends_review_context_for_skill_only_prompt() {
    let tmp = TempDir::new().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let skill_dir = tmp.path().join(".csa").join("skills").join("demo");
    fs::create_dir_all(&skill_dir).expect("create skill dir");
    fs::write(skill_dir.join("SKILL.md"), "demo skill body").expect("write SKILL.md");

    let session_id = "01KAS6M5XG7V4M4M6YDRS7P8R4";
    let session_dir = csa_session::get_session_dir(tmp.path(), session_id).expect("session dir");
    fs::create_dir_all(session_dir.join("output")).expect("create output dir");
    fs::write(
        session_dir.join("output").join("summary.md"),
        "Summary line\n",
    )
    .expect("write summary");

    let skill_resolution =
        resolve_skill_and_prompt(Some("demo"), None, None, None, None, tmp.path())
            .expect("resolve skill prompt");

    let prompt_text =
        finalize_prompt_text(tmp.path(), skill_resolution.prompt_text, Some(session_id))
            .expect("finalize prompt text");

    let expected_review_context_prefix = format!(
        "<csa-review-context session=\"{session_id}\">\n<!-- summary.md -->\nSummary line\n"
    );
    assert!(prompt_text.starts_with(crate::run_helpers::ATOMIC_COMMIT_DISCIPLINE_PREAMBLE));
    let review_context_pos = prompt_text
        .find(&expected_review_context_prefix)
        .expect("review context should appear");
    let original_prompt_pos = prompt_text
        .find("<original-prompt>\n<skill-mode>executor</skill-mode>\n")
        .expect("original prompt should appear");
    assert!(
        prompt_text.find("<atomic-commit-discipline>").unwrap() < review_context_pos,
        "atomic commit discipline must come before review context"
    );
    assert!(
        review_context_pos < original_prompt_pos,
        "review context must remain before original prompt"
    );
    assert!(prompt_text.contains("<original-prompt>\n<skill-mode>executor</skill-mode>\n"));
    assert!(prompt_text.contains("demo skill body"));
    assert!(prompt_text.ends_with("</original-prompt>\n"));
}

#[test]
fn resolve_run_tier_context_keeps_active_strategy_tier() {
    let (tier_auto_select, failover_on_crash_enabled, resolved_tier_name) =
        resolve_run_tier_context(
            None,
            "codex",
            Some("tier-3-complex".to_string()),
            None,
            false,
            false,
            false,
        );

    assert!(tier_auto_select);
    assert!(failover_on_crash_enabled);
    assert_eq!(resolved_tier_name.as_deref(), Some("tier-3-complex"));
}

#[test]
fn resolve_run_tier_context_drops_bypassed_tier() {
    let (tier_auto_select, failover_on_crash_enabled, resolved_tier_name) =
        resolve_run_tier_context(
            None,
            "codex",
            Some("tier-3-complex".to_string()),
            Some("tier-2-standard".to_string()),
            true,
            false,
            true,
        );

    assert!(!tier_auto_select);
    assert!(!failover_on_crash_enabled);
    assert!(resolved_tier_name.is_none());
}

#[test]
fn resolve_run_tier_context_restores_fallback_tier_for_auto_routing() {
    let (tier_auto_select, failover_on_crash_enabled, resolved_tier_name) =
        resolve_run_tier_context(
            None,
            "codex",
            None,
            Some("tier-3-complex".to_string()),
            false,
            false,
            false,
        );

    assert!(tier_auto_select);
    assert!(failover_on_crash_enabled);
    assert_eq!(resolved_tier_name.as_deref(), Some("tier-3-complex"));
}

#[test]
fn resolve_run_tier_context_does_not_restore_fallback_for_user_explicit_tool() {
    let (tier_auto_select, failover_on_crash_enabled, resolved_tier_name) =
        resolve_run_tier_context(
            None,
            "codex",
            None,
            Some("tier-3-complex".to_string()),
            false,
            false,
            true,
        );

    assert!(!tier_auto_select);
    assert!(!failover_on_crash_enabled);
    assert!(resolved_tier_name.is_none());
}

#[test]
fn resolve_run_tier_context_enables_crash_failover_for_explicit_tool_in_tier() {
    let config = make_test_config();
    let (tier_auto_select, failover_on_crash_enabled, resolved_tier_name) =
        resolve_run_tier_context(
            Some(&config),
            "codex",
            Some("tier-3-complex".to_string()),
            None,
            false,
            false,
            true,
        );

    assert!(!tier_auto_select);
    assert!(failover_on_crash_enabled);
    assert_eq!(resolved_tier_name.as_deref(), Some("tier-3-complex"));
}

#[test]
fn resolve_run_tier_context_drops_tier_for_explicit_model_spec() {
    let (tier_auto_select, failover_on_crash_enabled, resolved_tier_name) =
        resolve_run_tier_context(
            None,
            "codex",
            None,
            Some("tier-3-complex".to_string()),
            false,
            true,
            false,
        );

    assert!(!tier_auto_select);
    assert!(!failover_on_crash_enabled);
    assert!(resolved_tier_name.is_none());
}
