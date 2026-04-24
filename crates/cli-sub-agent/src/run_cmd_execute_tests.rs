use super::{
    RunModelSelectionFlags, finalize_prompt_text, resolve_primary_writer_spec_for_run,
    resolve_run_tier_context,
};
use crate::run_cmd_tool_selection::{resolve_skill_and_prompt, resolve_tool_by_strategy};
use crate::test_session_sandbox::ScopedSessionSandbox;
use chrono::Utc;
use csa_config::global::PreferencesConfig;
use csa_config::{GlobalConfig, ProjectConfig, ProjectMeta, TierConfig, TierStrategy};
use csa_core::types::{ToolName, ToolSelectionStrategy};
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
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    }
}

fn make_config_with_primary_writer_spec(spec: &str) -> ProjectConfig {
    let mut config = make_test_config();
    config.preferences = Some(PreferencesConfig {
        primary_writer_spec: Some(spec.to_string()),
        ..Default::default()
    });
    config
}

fn atomic_commit_block<'a>(prompt: &'a str, user_task_marker: &str) -> &'a str {
    let preamble_start = prompt
        .find("<atomic-commit-discipline>")
        .expect("preamble must exist");
    let user_task_pos = prompt
        .find(user_task_marker)
        .expect("user task marker must exist");
    &prompt[preamble_start..user_task_pos]
}

#[test]
fn finalize_prompt_text_prepends_atomic_commit_preamble() {
    let tmp = TempDir::new().expect("tempdir");
    let mut sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    sandbox.track_env("CSA_DEPTH");
    sandbox.track_env("CSA_SESSION_ID");
    // SAFETY: ScopedSessionSandbox holds TEST_ENV_LOCK for the full test.
    unsafe {
        std::env::remove_var("CSA_DEPTH");
        std::env::remove_var("CSA_SESSION_ID");
    }

    let result = finalize_prompt_text(tmp.path(), "user task".to_string(), None).expect("finalize");
    let preamble_body = atomic_commit_block(&result, "user task");

    assert!(
        result.contains("<atomic-commit-discipline>"),
        "preamble must appear in finalized prompt; got: {result}"
    );
    assert!(result.contains("user task"));
    assert!(
        preamble_body.contains("/commit"),
        "preamble must direct agents to the /commit skill; got: {preamble_body}"
    );
    assert!(
        !preamble_body.contains("git commit -m"),
        "preamble must not instruct manual git commit; got: {preamble_body}"
    );
    assert!(
        !preamble_body.contains("git add"),
        "preamble must not instruct manual git add; got: {preamble_body}"
    );
    assert!(
        preamble_body.contains("session output directory IS persisted")
            && preamble_body.contains("$CSA_SESSION_DIR/output/<name>.md"),
        "preamble must document persisted artifact location; got: {preamble_body}"
    );
    assert!(
        result.find("<atomic-commit-discipline>").unwrap() < result.find("user task").unwrap(),
        "preamble must come BEFORE the user prompt"
    );
}

#[test]
fn finalize_prompt_text_uses_subprocess_atomic_commit_preamble_when_csa_depth_positive() {
    let tmp = TempDir::new().expect("tempdir");
    let mut sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    sandbox.track_env("CSA_DEPTH");
    sandbox.track_env("CSA_SESSION_ID");
    // SAFETY: ScopedSessionSandbox holds TEST_ENV_LOCK for the full test.
    unsafe {
        std::env::set_var("CSA_DEPTH", "1");
        std::env::remove_var("CSA_SESSION_ID");
    }

    let result =
        finalize_prompt_text(tmp.path(), "subprocess task".to_string(), None).expect("finalize");
    let preamble_body = atomic_commit_block(&result, "subprocess task");

    assert!(
        preamble_body.contains("git commit -m"),
        "subprocess preamble must instruct direct git commit; got: {preamble_body}"
    );
    assert!(
        preamble_body.contains("git add"),
        "subprocess preamble must instruct direct git add; got: {preamble_body}"
    );
    assert!(
        !preamble_body.contains("/commit"),
        "subprocess preamble must not reference /commit; got: {preamble_body}"
    );
    assert!(
        preamble_body.contains("session output directory IS persisted")
            && preamble_body.contains("$CSA_SESSION_DIR/output/<name>.md"),
        "subprocess preamble must document persisted artifact location; got: {preamble_body}"
    );
}

#[test]
fn finalize_prompt_text_uses_main_agent_preamble_when_csa_depth_missing() {
    let tmp = TempDir::new().expect("tempdir");
    let mut sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    sandbox.track_env("CSA_DEPTH");
    sandbox.track_env("CSA_SESSION_ID");
    // SAFETY: ScopedSessionSandbox holds TEST_ENV_LOCK for the full test.
    unsafe {
        std::env::remove_var("CSA_DEPTH");
        std::env::remove_var("CSA_SESSION_ID");
    }

    let result =
        finalize_prompt_text(tmp.path(), "main agent task".to_string(), None).expect("finalize");
    let preamble_body = atomic_commit_block(&result, "main agent task");

    assert!(
        preamble_body.contains("/commit"),
        "main-agent preamble must reference /commit; got: {preamble_body}"
    );
    assert!(
        !preamble_body.contains("git commit -m"),
        "main-agent preamble must not instruct manual git commit; got: {preamble_body}"
    );
}

#[test]
fn finalize_prompt_text_uses_main_agent_preamble_when_csa_depth_zero() {
    let tmp = TempDir::new().expect("tempdir");
    let mut sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    sandbox.track_env("CSA_DEPTH");
    sandbox.track_env("CSA_SESSION_ID");
    // SAFETY: ScopedSessionSandbox holds TEST_ENV_LOCK for the full test.
    unsafe {
        std::env::set_var("CSA_DEPTH", "0");
        std::env::remove_var("CSA_SESSION_ID");
    }

    let result =
        finalize_prompt_text(tmp.path(), "depth zero task".to_string(), None).expect("finalize");
    let preamble_body = atomic_commit_block(&result, "depth zero task");

    assert!(
        preamble_body.contains("/commit"),
        "CSA_DEPTH=0 must still use main-agent /commit instructions; got: {preamble_body}"
    );
    assert!(
        !preamble_body.contains("git commit -m"),
        "CSA_DEPTH=0 must not use subprocess git commit instructions; got: {preamble_body}"
    );
}

#[test]
fn finalize_prompt_text_uses_subprocess_preamble_when_only_session_id_is_set() {
    let tmp = TempDir::new().expect("tempdir");
    let mut sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    sandbox.track_env("CSA_DEPTH");
    sandbox.track_env("CSA_SESSION_ID");
    // Treat CSA_SESSION_ID alone as subprocess so detached child contexts still avoid the
    // unavailable /commit slash-command path.
    // SAFETY: ScopedSessionSandbox holds TEST_ENV_LOCK for the full test.
    unsafe {
        std::env::remove_var("CSA_DEPTH");
        std::env::set_var("CSA_SESSION_ID", "01KPSX30G8HRHM5RHGBDB2XPSA");
    }

    let result =
        finalize_prompt_text(tmp.path(), "session-id task".to_string(), None).expect("finalize");
    let preamble_body = atomic_commit_block(&result, "session-id task");

    assert!(
        preamble_body.contains("git commit -m"),
        "CSA_SESSION_ID fallback must use subprocess git commit instructions; got: {preamble_body}"
    );
    assert!(
        !preamble_body.contains("/commit"),
        "CSA_SESSION_ID fallback must not reference /commit; got: {preamble_body}"
    );
}

#[test]
fn intent_classifier_sees_original_prompt_not_preamble() {
    let tmp = TempDir::new().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let original = "Review auth flow and report issues";
    let classified = crate::run_helpers::resolve_task_edit_requirement(None, original);

    assert_ne!(
        classified,
        Some(true),
        "sanity: original prompt must not be treated as mutating before preamble injection"
    );

    let final_prompt =
        finalize_prompt_text(tmp.path(), original.to_string(), None).expect("finalize");
    assert!(
        final_prompt.contains("<atomic-commit-discipline>"),
        "preamble must still be in final prompt"
    );
    assert_eq!(
        crate::run_helpers::resolve_task_edit_requirement(None, &final_prompt),
        Some(true),
        "sanity: classifying the finalized prompt would regress routing"
    );
    assert_eq!(
        classified,
        crate::run_helpers::resolve_task_edit_requirement(None, original),
        "run flow must preserve pre-preamble classification for routing"
    );
}

#[test]
fn finalize_prompt_text_keeps_read_only_original_prompt_classification() {
    let tmp = TempDir::new().expect("tempdir");
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let original = "Review auth flow and report issues in read-only mode";
    let final_prompt =
        finalize_prompt_text(tmp.path(), original.to_string(), None).expect("finalize");

    assert!(
        final_prompt.contains("<atomic-commit-discipline>"),
        "preamble must be present in finalized prompt"
    );
    assert!(
        final_prompt.contains(original),
        "finalized prompt must preserve original prompt text"
    );
    assert_eq!(
        crate::run_helpers::resolve_task_edit_requirement(None, original),
        Some(false),
        "original prompt must stay read-only even when finalized prompt contains preamble"
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
    assert!(prompt_text.starts_with(crate::run_helpers::atomic_commit_discipline_preamble()));
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

#[test]
fn primary_writer_spec_seeds_run_without_model_selecting_flags_and_bypasses_tiers() {
    let tmp = TempDir::new().expect("tempdir");
    let config = make_config_with_primary_writer_spec("codex/openai/gpt-5.4/high");
    let global_config = GlobalConfig::default();

    let spec = resolve_primary_writer_spec_for_run(
        RunModelSelectionFlags::default(),
        Some(&config),
        &global_config,
    )
    .expect("primary writer spec should apply");
    let resolution = resolve_tool_by_strategy(
        &ToolSelectionStrategy::AnyAvailable,
        Some(&spec),
        None,
        Some(&config),
        &global_config,
        tmp.path(),
        false,
        false,
        false,
        None,
        false,
    )
    .expect("primary writer spec should resolve as model-spec");

    assert_eq!(resolution.tool, ToolName::Codex);
    assert_eq!(resolution.model_spec.as_deref(), Some(spec.as_str()));
    assert!(
        resolution.resolved_tier_name.is_none(),
        "synthetic model-spec must keep --model-spec tier bypass semantics"
    );
}

#[test]
fn primary_writer_spec_prefers_project_over_global() {
    let config = make_config_with_primary_writer_spec("codex/openai/gpt-5.4/high");
    let mut global_config = GlobalConfig::default();
    global_config.preferences.primary_writer_spec =
        Some("claude-code/anthropic/default/xhigh".to_string());

    let spec = resolve_primary_writer_spec_for_run(
        RunModelSelectionFlags::default(),
        Some(&config),
        &global_config,
    );

    assert_eq!(spec.as_deref(), Some("codex/openai/gpt-5.4/high"));
}

#[test]
fn primary_writer_spec_is_suppressed_by_any_model_selecting_flag() {
    let config = make_config_with_primary_writer_spec("codex/openai/gpt-5.4/high");
    let global_config = GlobalConfig::default();
    let cases = [
        RunModelSelectionFlags {
            tool: true,
            ..Default::default()
        },
        RunModelSelectionFlags {
            auto_route: true,
            ..Default::default()
        },
        RunModelSelectionFlags {
            skill: true,
            ..Default::default()
        },
        RunModelSelectionFlags {
            model_spec: true,
            ..Default::default()
        },
        RunModelSelectionFlags {
            model: true,
            ..Default::default()
        },
        RunModelSelectionFlags {
            thinking: true,
            ..Default::default()
        },
        RunModelSelectionFlags {
            tier: true,
            ..Default::default()
        },
        RunModelSelectionFlags {
            hint_difficulty: true,
            ..Default::default()
        },
    ];

    for flags in cases {
        let spec = resolve_primary_writer_spec_for_run(flags, Some(&config), &global_config);
        assert!(
            spec.is_none(),
            "flags should suppress primary writer: {flags:?}"
        );
    }
}
