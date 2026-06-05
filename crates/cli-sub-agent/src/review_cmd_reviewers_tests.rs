use super::{
    AutoReviewerRequest, resolve_auto_reviewer_selection, resolve_effective_reviewer_selection,
    resolve_multi_reviewer_pool,
};
use crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV;
use crate::test_env_lock::TEST_ENV_LOCK;
use csa_config::config::TierConfig;
use csa_config::{
    GlobalConfig, ProjectConfig, ProjectMeta, ResourcesConfig, ReviewConfig, TierStrategy,
    ToolConfig, ToolSelection,
};
use csa_core::types::ToolName;
use std::collections::HashMap;
use tokio::sync::OwnedMutexGuard;

struct ScopedEnvVarRestore {
    key: &'static str,
    original: Option<String>,
}

impl ScopedEnvVarRestore {
    fn set(key: &'static str, value: &str) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

impl Drop for ScopedEnvVarRestore {
    fn drop(&mut self) {
        // SAFETY: restoration of test-scoped env mutation.
        unsafe {
            match self.original.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

fn assume_review_tools_available() -> (OwnedMutexGuard<()>, ScopedEnvVarRestore) {
    (
        TEST_ENV_LOCK.clone().blocking_lock_owned(),
        ScopedEnvVarRestore::set(TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1"),
    )
}

fn project_config_with_tier(models: &[&str]) -> ProjectConfig {
    let mut tool_map = HashMap::new();
    for tool in csa_config::global::all_known_tools() {
        tool_map.insert(
            tool.as_str().to_string(),
            ToolConfig {
                enabled: false,
                restrictions: None,
                suppress_notify: true,
                ..Default::default()
            },
        );
    }

    for tool_name in models.iter().filter_map(|model| model.split('/').next()) {
        tool_map.insert(
            tool_name.to_string(),
            ToolConfig {
                enabled: true,
                restrictions: None,
                suppress_notify: true,
                ..Default::default()
            },
        );
    }

    let mut tiers = HashMap::new();
    tiers.insert(
        "quality".to_string(),
        TierConfig {
            description: "Test tier".to_string(),
            models: models.iter().map(|model| (*model).to_string()).collect(),
            strategy: TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );

    ProjectConfig {
        schema_version: 1,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: tool_map,
        review: None,
        debate: None,
        tiers,
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
    }
}

fn project_config_with_tier_and_review_tool(models: &[&str], tool: ToolSelection) -> ProjectConfig {
    let mut config = project_config_with_tier(models);
    config.review = Some(ReviewConfig {
        tool,
        ..ReviewConfig::default()
    });
    config
}

fn distinct_model_family_count(tools: &[ToolName]) -> usize {
    let mut families = Vec::new();
    for tool in tools {
        let family = tool.model_family();
        if !families.contains(&family) {
            families.push(family);
        }
    }
    families.len()
}

#[test]
fn auto_reviewer_selection_skips_single_tool_tier() {
    let (_env_lock, _available_guard) = assume_review_tools_available();
    let config = project_config_with_tier(&["codex/openai/gpt-5.4/high"]);
    let global = GlobalConfig::default();

    let selection = resolve_auto_reviewer_selection(&AutoReviewerRequest {
        requested_reviewers: 1,
        explicit_reviewer_count: false,
        single: false,
        scope_is_range: true,
        large_diff_auto_escalation: false,
        explicit_tool: None,
        explicit_model_spec: None,
        primary_tool: ToolName::Codex,
        resolved_tier_name: Some("quality"),
        config: Some(&config),
        global_config: &global,
    });

    assert!(selection.is_none());
}

#[test]
fn auto_reviewer_selection_uses_all_distinct_tier_tools_up_to_cap() {
    let (_env_lock, _available_guard) = assume_review_tools_available();
    let config = project_config_with_tier(&[
        "codex/openai/gpt-5.4/high",
        "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
        "opencode/openrouter/sonnet/high",
        "claude-code/anthropic/sonnet/xhigh",
    ]);
    let global = GlobalConfig::default();

    let selection = resolve_auto_reviewer_selection(&AutoReviewerRequest {
        requested_reviewers: 1,
        explicit_reviewer_count: false,
        single: false,
        scope_is_range: true,
        large_diff_auto_escalation: false,
        explicit_tool: None,
        explicit_model_spec: None,
        primary_tool: ToolName::GeminiCli,
        resolved_tier_name: Some("quality"),
        config: Some(&config),
        global_config: &global,
    })
    .expect("multi-tool tier should auto-select reviewers");

    assert_eq!(selection.reviewers, 3);
    assert_eq!(
        selection.selected_tools,
        vec![ToolName::GeminiCli, ToolName::Codex, ToolName::Opencode]
    );
    assert_eq!(distinct_model_family_count(&selection.selected_tools), 3);
}

#[test]
fn large_diff_auto_escalation_selects_heterogeneous_reviewers_for_non_range_scope() {
    let (_env_lock, _available_guard) = assume_review_tools_available();
    let config = project_config_with_tier(&[
        "codex/openai/gpt-5.4/high",
        "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
    ]);
    let global = GlobalConfig::default();

    let selection = resolve_auto_reviewer_selection(&AutoReviewerRequest {
        requested_reviewers: 1,
        explicit_reviewer_count: false,
        single: false,
        scope_is_range: false,
        large_diff_auto_escalation: true,
        explicit_tool: None,
        explicit_model_spec: None,
        primary_tool: ToolName::Codex,
        resolved_tier_name: Some("quality"),
        config: Some(&config),
        global_config: &global,
    })
    .expect("large non-range diff should auto-select heterogeneous reviewers");

    assert!(selection.reviewers >= 2);
    assert!(distinct_model_family_count(&selection.selected_tools) >= 2);
    assert_eq!(
        selection.selected_tools,
        vec![ToolName::Codex, ToolName::GeminiCli]
    );
}

#[test]
fn auto_range_reviewer_roster_prefers_review_tool_without_filtering_tier() {
    let (_env_lock, _available_guard) = assume_review_tools_available();
    let config = project_config_with_tier_and_review_tool(
        &[
            "codex/openai/gpt-5.4/high",
            "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
        ],
        ToolSelection::Whitelist(vec!["codex".to_string()]),
    );
    let global = GlobalConfig::default();

    let pool = resolve_multi_reviewer_pool(
        2,
        None,
        None,
        ToolName::Codex,
        Some("quality"),
        Some(&config),
        &global,
    )
    .expect("preferred single-tool roster should resolve");

    assert_eq!(
        pool.reviewer_tools,
        vec![ToolName::Codex, ToolName::GeminiCli]
    );
    assert!(
        pool.tier_reviewer_specs
            .iter()
            .any(|resolution| resolution.tool == ToolName::GeminiCli)
    );
}

fn assert_auto_execution_uses_selected_heterogeneous_roster(large_diff_auto_escalation: bool) {
    let (_env_lock, _available_guard) = assume_review_tools_available();
    let config = project_config_with_tier(&[
        "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
        "antigravity-cli/google/gemini-3.1-pro-preview/xhigh",
        "codex/openai/gpt-5.4/high",
    ]);
    let global = GlobalConfig::default();
    let effective = resolve_effective_reviewer_selection(&AutoReviewerRequest {
        requested_reviewers: 1,
        explicit_reviewer_count: false,
        single: false,
        scope_is_range: !large_diff_auto_escalation,
        large_diff_auto_escalation,
        explicit_tool: None,
        explicit_model_spec: None,
        primary_tool: ToolName::GeminiCli,
        resolved_tier_name: Some("quality"),
        config: Some(&config),
        global_config: &global,
    });
    let selected_tools = effective
        .selected_tools
        .expect("auto path should carry the roster");
    assert_eq!(effective.reviewers, selected_tools.len());
    let pool = resolve_multi_reviewer_pool(
        effective.reviewers,
        Some(&selected_tools),
        None,
        ToolName::GeminiCli,
        Some("quality"),
        Some(&config),
        &global,
    )
    .expect("auto-selected reviewer roster should resolve for execution");
    let same_family_pair = [ToolName::GeminiCli, ToolName::AntigravityCli];
    assert_eq!(pool.reviewer_tools, selected_tools);
    assert!(distinct_model_family_count(&pool.reviewer_tools) >= 2);
    assert_eq!(
        &pool.reviewer_tools[..2],
        [ToolName::GeminiCli, ToolName::Codex].as_slice()
    );
    assert_ne!(&pool.reviewer_tools[..2], same_family_pair.as_slice());
}

#[test]
fn auto_execution_uses_selected_heterogeneous_reviewer_roster_for_range_and_large_diff() {
    assert_auto_execution_uses_selected_heterogeneous_roster(false);
    assert_auto_execution_uses_selected_heterogeneous_roster(true);
}

#[test]
fn auto_reviewer_selection_respects_single_flag() {
    let (_env_lock, _available_guard) = assume_review_tools_available();
    let config = project_config_with_tier(&[
        "codex/openai/gpt-5.4/high",
        "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
    ]);
    let global = GlobalConfig::default();

    let selection = resolve_auto_reviewer_selection(&AutoReviewerRequest {
        requested_reviewers: 1,
        explicit_reviewer_count: false,
        single: true,
        scope_is_range: true,
        large_diff_auto_escalation: true,
        explicit_tool: None,
        explicit_model_spec: None,
        primary_tool: ToolName::Codex,
        resolved_tier_name: Some("quality"),
        config: Some(&config),
        global_config: &global,
    });

    assert!(selection.is_none());
}

#[test]
fn auto_reviewer_selection_respects_explicit_reviewer_override() {
    let (_env_lock, _available_guard) = assume_review_tools_available();
    let config = project_config_with_tier(&[
        "codex/openai/gpt-5.4/high",
        "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
    ]);
    let global = GlobalConfig::default();

    let selection = resolve_auto_reviewer_selection(&AutoReviewerRequest {
        requested_reviewers: 1,
        explicit_reviewer_count: true,
        single: false,
        scope_is_range: true,
        large_diff_auto_escalation: true,
        explicit_tool: None,
        explicit_model_spec: None,
        primary_tool: ToolName::Codex,
        resolved_tier_name: Some("quality"),
        config: Some(&config),
        global_config: &global,
    });

    assert!(selection.is_none());
}

#[test]
fn large_diff_auto_escalation_respects_explicit_reviewer_count() {
    let (_env_lock, _available_guard) = assume_review_tools_available();
    let config = project_config_with_tier(&[
        "codex/openai/gpt-5.4/high",
        "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
        "opencode/openrouter/sonnet/high",
    ]);
    let global = GlobalConfig::default();

    let selection = resolve_effective_reviewer_selection(&AutoReviewerRequest {
        requested_reviewers: 2,
        explicit_reviewer_count: true,
        single: false,
        scope_is_range: false,
        large_diff_auto_escalation: true,
        explicit_tool: None,
        explicit_model_spec: None,
        primary_tool: ToolName::Codex,
        resolved_tier_name: Some("quality"),
        config: Some(&config),
        global_config: &global,
    });

    assert_eq!(selection.reviewers, 2);
    assert!(selection.selected_tools.is_none());
}

#[test]
fn auto_reviewer_selection_respects_explicit_model_spec_override() {
    let (_env_lock, _available_guard) = assume_review_tools_available();
    let config = project_config_with_tier(&[
        "codex/openai/gpt-5.4/high",
        "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
    ]);
    let global = GlobalConfig::default();

    let selection = resolve_auto_reviewer_selection(&AutoReviewerRequest {
        requested_reviewers: 1,
        explicit_reviewer_count: false,
        single: false,
        scope_is_range: true,
        large_diff_auto_escalation: true,
        explicit_tool: None,
        explicit_model_spec: Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh"),
        primary_tool: ToolName::Codex,
        resolved_tier_name: Some("quality"),
        config: Some(&config),
        global_config: &global,
    });

    assert!(selection.is_none());
}

#[test]
fn auto_reviewer_selection_respects_explicit_tool_override() {
    let (_env_lock, _available_guard) = assume_review_tools_available();
    let config = project_config_with_tier(&[
        "codex/openai/gpt-5.4/high",
        "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
    ]);
    let global = GlobalConfig::default();

    let selection = resolve_auto_reviewer_selection(&AutoReviewerRequest {
        requested_reviewers: 1,
        explicit_reviewer_count: false,
        single: false,
        scope_is_range: true,
        large_diff_auto_escalation: true,
        explicit_tool: Some(ToolName::Codex),
        explicit_model_spec: None,
        primary_tool: ToolName::Codex,
        resolved_tier_name: Some("quality"),
        config: Some(&config),
        global_config: &global,
    });

    assert!(selection.is_none());
}

#[test]
fn large_diff_auto_escalation_requires_two_model_families() {
    let (_env_lock, _available_guard) = assume_review_tools_available();
    let config = project_config_with_tier(&[
        "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
        "antigravity-cli/google/gemini-3.1-pro-preview/xhigh",
    ]);
    let global = GlobalConfig::default();

    let selection = resolve_auto_reviewer_selection(&AutoReviewerRequest {
        requested_reviewers: 1,
        explicit_reviewer_count: false,
        single: false,
        scope_is_range: false,
        large_diff_auto_escalation: true,
        explicit_tool: None,
        explicit_model_spec: None,
        primary_tool: ToolName::GeminiCli,
        resolved_tier_name: Some("quality"),
        config: Some(&config),
        global_config: &global,
    });

    assert!(selection.is_none());
}

#[test]
fn auto_reviewer_selection_skips_non_range_scope() {
    let (_env_lock, _available_guard) = assume_review_tools_available();
    let config = project_config_with_tier(&[
        "codex/openai/gpt-5.4/high",
        "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
    ]);
    let global = GlobalConfig::default();

    let selection = resolve_auto_reviewer_selection(&AutoReviewerRequest {
        requested_reviewers: 1,
        explicit_reviewer_count: false,
        single: false,
        scope_is_range: false,
        large_diff_auto_escalation: false,
        explicit_tool: None,
        explicit_model_spec: None,
        primary_tool: ToolName::Codex,
        resolved_tier_name: Some("quality"),
        config: Some(&config),
        global_config: &global,
    });

    assert!(selection.is_none());
}
