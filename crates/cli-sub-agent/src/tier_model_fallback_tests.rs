use super::{
    TierAttemptFailure, classify_next_model_failure_with_elapsed, earliest_backend_reset_window,
    format_all_models_failed_reason_with_reset, opaque_total_exhaustion_message,
    ordered_tier_candidates, parse_backend_reset_duration,
};
use csa_config::{GlobalConfig, ProjectConfig, ToolConfig, global::GlobalToolConfig};
use csa_core::types::ToolName;
use std::collections::HashMap;
use std::time::Duration;

fn project_config_with_tier(
    tier_name: &str,
    models: &[&str],
    enabled_tools: &[&str],
) -> ProjectConfig {
    let mut tool_map = HashMap::new();
    for tool in csa_config::global::all_known_tools() {
        let name = tool.as_str();
        tool_map.insert(
            name.to_string(),
            ToolConfig {
                enabled: enabled_tools.contains(&name),
                ..Default::default()
            },
        );
    }

    let mut cfg = ProjectConfig {
        schema_version: csa_config::config::CURRENT_SCHEMA_VERSION,
        project: csa_config::ProjectMeta {
            name: "test".to_string(),
            created_at: chrono::Utc::now(),
            max_recursion_depth: 5,
        },
        resources: csa_config::ResourcesConfig::default(),
        acp: Default::default(),
        tools: tool_map,
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };
    cfg.tiers.insert(
        tier_name.to_string(),
        csa_config::config::TierConfig {
            description: "Test tier".to_string(),
            models: models.iter().map(|spec| (*spec).to_string()).collect(),
            strategy: csa_config::TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );
    cfg
}

fn global_config_preferring_openai_compat() -> GlobalConfig {
    let mut global = GlobalConfig::default();
    global.preferences.tool_priority = vec!["openai-compat".to_string()];
    global
}

fn global_config_with_openai_compat_env() -> GlobalConfig {
    let mut global = global_config_preferring_openai_compat();
    global.tools.insert(
        "openai-compat".to_string(),
        GlobalToolConfig {
            env: HashMap::from([
                (
                    "OPENAI_COMPAT_BASE_URL".to_string(),
                    "http://localhost:8317".to_string(),
                ),
                ("OPENAI_COMPAT_API_KEY".to_string(), "test-key".to_string()),
                ("OPENAI_COMPAT_MODEL".to_string(), "local-model".to_string()),
            ]),
            ..Default::default()
        },
    );
    global
}

#[test]
fn tier_fallback_prefers_original_tool_then_keeps_full_tier() {
    let _availability = crate::test_env_lock::ScopedEnvVarRestore::set(
        crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV,
        "1",
    );
    let cfg = project_config_with_tier(
        "quality",
        &[
            "codex/openai/gpt-5.4/high",
            "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
            "claude-code/anthropic/sonnet-4.6/xhigh",
        ],
        &["codex", "gemini-cli", "claude-code"],
    );

    let candidates = ordered_tier_candidates(
        ToolName::Codex,
        Some("codex/openai/gpt-5.4/high"),
        Some("quality"),
        Some(&cfg),
        None,
        true,
        &["codex".to_string()],
    );

    assert_eq!(
        candidates,
        vec![
            (
                ToolName::Codex,
                Some("codex/openai/gpt-5.4/high".to_string()),
            ),
            (
                ToolName::GeminiCli,
                Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string()),
            ),
            (
                ToolName::ClaudeCode,
                Some("claude-code/anthropic/sonnet-4.6/xhigh".to_string()),
            ),
        ]
    );
}

#[test]
fn no_tier_fallback_uses_global_tool_priority() {
    let _availability = crate::test_env_lock::ScopedEnvVarRestore::set(
        crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV,
        "1",
    );
    let mut global = GlobalConfig::default();
    global.preferences.tool_priority = vec!["claude-code".to_string(), "gemini-cli".to_string()];
    let candidates =
        ordered_tier_candidates(ToolName::Codex, None, None, None, Some(&global), true, &[]);

    assert!(candidates.starts_with(&[
        (ToolName::Codex, None),
        (ToolName::ClaudeCode, None),
        (ToolName::GeminiCli, None),
    ]));
}

#[test]
fn no_tier_fallback_keeps_global_env_only_openai_compat() {
    let _lock = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let _availability = crate::test_env_lock::ScopedEnvVarRestore::set(
        crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV,
        "1",
    );
    let _base = crate::test_env_lock::ScopedEnvVarRestore::unset("OPENAI_COMPAT_BASE_URL");
    let _key = crate::test_env_lock::ScopedEnvVarRestore::unset("OPENAI_COMPAT_API_KEY");
    let _model = crate::test_env_lock::ScopedEnvVarRestore::unset("OPENAI_COMPAT_MODEL");
    let cfg = project_config_with_tier(
        "quality",
        &[
            "openai-compat/openai/gpt-5/high",
            "codex/openai/gpt-5.4/high",
        ],
        &["openai-compat", "codex"],
    );
    let global = global_config_with_openai_compat_env();

    let candidates = ordered_tier_candidates(
        ToolName::Codex,
        None,
        None,
        Some(&cfg),
        Some(&global),
        true,
        &[],
    );

    assert_eq!(
        candidates.get(1),
        Some(&(ToolName::OpenaiCompat, None)),
        "global tool env must make openai-compat a valid non-tier fallback candidate"
    );
}

#[test]
fn no_tier_fallback_skips_unconfigured_openai_compat() {
    let _lock = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let _availability = crate::test_env_lock::ScopedEnvVarRestore::set(
        crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV,
        "1",
    );
    let _base = crate::test_env_lock::ScopedEnvVarRestore::unset("OPENAI_COMPAT_BASE_URL");
    let _key = crate::test_env_lock::ScopedEnvVarRestore::unset("OPENAI_COMPAT_API_KEY");
    let _model = crate::test_env_lock::ScopedEnvVarRestore::unset("OPENAI_COMPAT_MODEL");
    let cfg = project_config_with_tier(
        "quality",
        &[
            "openai-compat/openai/gpt-5/high",
            "codex/openai/gpt-5.4/high",
        ],
        &["openai-compat", "codex"],
    );
    let global = global_config_preferring_openai_compat();

    let candidates = ordered_tier_candidates(
        ToolName::Codex,
        None,
        None,
        Some(&cfg),
        Some(&global),
        true,
        &[],
    );

    assert!(
        candidates
            .iter()
            .all(|(tool, _)| *tool != ToolName::OpenaiCompat),
        "unconfigured openai-compat must be skipped before non-tier fallback routing"
    );
}

#[test]
fn opaque_total_exhaustion_uses_provider_reset_window() {
    let failure_reason = "all tier-4-critical models failed: \
            gemini-cli/google/gemini-3.1-pro-preview/xhigh=auth_unavailable; quota will reset after 14h, \
            codex/openai/gpt-5.5/xhigh=HTTP 429; reset in 1h";

    assert_eq!(
        opaque_total_exhaustion_message(Some("auth_unavailable; HTTP 429"), Some(failure_reason))
            .as_deref(),
        Some(
            "review unavailable: all tier-4-critical backends rate-limited; earliest reset ~1h 0m"
        )
    );
}

#[test]
fn opaque_total_exhaustion_omits_unparseable_reset_window() {
    let failure_reason = "all tier-4-critical models failed: \
            gemini-cli/google/gemini-3.1-pro-preview/xhigh=auth_unavailable; reset pending, \
            codex/openai/gpt-5.5/xhigh=HTTP 429; reset unknown";

    assert_eq!(
        opaque_total_exhaustion_message(Some("auth_unavailable; HTTP 429"), Some(failure_reason))
            .as_deref(),
        Some("review unavailable: all tier-4-critical backends rate-limited")
    );
}

#[test]
fn all_models_failed_reason_uses_earliest_backend_reset_window() {
    let reset_windows = [
        parse_backend_reset_duration("provider said quota will reset after 14h"),
        parse_backend_reset_duration("provider said reset in 1h"),
        parse_backend_reset_duration("provider said reset unknown"),
    ];
    let parseable_reset_windows = reset_windows.into_iter().flatten().collect::<Vec<_>>();
    let failures = vec![
        TierAttemptFailure {
            model_spec: "gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string(),
            reason: "auth_unavailable".to_string(),
            quota_exhausted: Some(false),
        },
        TierAttemptFailure {
            model_spec: "codex/openai/gpt-5.5/xhigh".to_string(),
            reason: "HTTP 429".to_string(),
            quota_exhausted: Some(false),
        },
    ];

    assert_eq!(
        format_all_models_failed_reason_with_reset(
            Some("tier-4-critical"),
            &failures,
            earliest_backend_reset_window(&parseable_reset_windows),
        )
        .as_deref(),
        Some(
            "all tier-4-critical models failed: \
gemini-cli/google/gemini-3.1-pro-preview/xhigh=auth_unavailable, \
codex/openai/gpt-5.5/xhigh=HTTP 429; earliest_reset=1h 0m"
        )
    );
}

/// The exact stderr gemini-cli emits when its key is rejected mid-run (a 400
/// from inside `ModelRouterService.route` once a session is established).
const GEMINI_400_API_KEY_INVALID_STDERR: &str = r#"Error talking to Gemini API in ModelRouterService.route: _ApiError: {"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","status":"INVALID_ARGUMENT","details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"API_KEY_INVALID","domain":"googleapis.com"}]}}"#;
const GEMINI_MANUAL_AUTHORIZATION_STDERR: &str = "\
Error: Manual authorization is required. \
Please run the Gemini CLI in an interactive terminal to log in.";

#[test]
fn issue_1848_midrun_gemini_api_key_invalid_advances_past_init_window() {
    // #1848: a key/OAuth-exhaustion 400 raised at 39s (after two earlier
    // dims succeeded) was NOT advancing because it classed as `HTTP 400`,
    // which is init-window-gated, and 39s > 30s. Now classed `auth_unavailable`,
    // it advances regardless of elapsed time.
    let detected = classify_next_model_failure_with_elapsed(
        "gemini-cli",
        GEMINI_400_API_KEY_INVALID_STDERR,
        "",
        1,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
        Some(Duration::from_secs(39)),
    )
    .expect("mid-run API_KEY_INVALID 400 must advance to the next tier candidate (#1848)");
    assert_eq!(detected.reason, "auth_unavailable");
    assert!(detected.advance_to_next_model);
    assert!(!detected.quota_exhausted);
}

#[test]
fn issue_1867_midrun_gemini_manual_authorization_advances_past_init_window() {
    let detected = classify_next_model_failure_with_elapsed(
        "gemini-cli",
        GEMINI_MANUAL_AUTHORIZATION_STDERR,
        "",
        1,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
        Some(Duration::from_secs(39)),
    )
    .expect("mid-run manual auth prompt must advance to the next tier candidate (#1867)");
    assert_eq!(detected.reason, "auth_unavailable");
    assert!(detected.advance_to_next_model);
    assert!(!detected.quota_exhausted);

    let generic_400 = classify_next_model_failure_with_elapsed(
        "gemini-cli",
        "Error: request failed with status: 400 Bad Request (malformed request payload)",
        "",
        1,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
        Some(Duration::from_secs(39)),
    );
    assert!(
        generic_400.is_none(),
        "a generic non-auth 400 past the init window must remain gated (#1736)"
    );
}

#[test]
fn issue_1848_midrun_generic_400_does_not_advance_past_init_window() {
    // Regression guard (#1736): a generic 400 with NO auth/key marker must
    // stay init-window-gated, so a genuine malformed-request failure at 39s
    // surfaces as an error rather than being silently masked by failover.
    let detected = classify_next_model_failure_with_elapsed(
        "gemini-cli",
        "Error: request failed with status: 400 Bad Request (malformed request payload)",
        "",
        1,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
        Some(Duration::from_secs(39)),
    );
    assert!(
        detected.is_none(),
        "a generic non-auth 400 past the init window must NOT be failover-masked (#1736)"
    );
}

#[test]
fn generic_400_within_init_window_still_advances() {
    // Unchanged behavior: a generic 400 raised at startup (within 30s) still
    // advances — only the post-init-window case is gated.
    let detected = classify_next_model_failure_with_elapsed(
        "gemini-cli",
        "Error: request failed with status: 400 Bad Request",
        "",
        1,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
        Some(Duration::from_secs(5)),
    )
    .expect("a startup 400 within the init window must still advance");
    assert_eq!(detected.reason, "HTTP 400");
    assert!(detected.advance_to_next_model);
}
