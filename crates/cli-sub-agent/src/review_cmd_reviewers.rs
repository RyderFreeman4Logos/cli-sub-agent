use anyhow::Result;
use tracing::{info, warn};

use crate::review_consensus::{build_reviewer_tools, validate_multi_reviewer_tier_pool};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;

const MAX_AUTO_HETEROGENEOUS_REVIEWERS: usize = 3;

pub(crate) struct AutoReviewerSelection {
    pub(crate) reviewers: usize,
    pub(crate) selected_tools: Vec<ToolName>,
}

pub(crate) struct AutoReviewerRequest<'a> {
    pub(crate) requested_reviewers: usize,
    pub(crate) explicit_reviewer_count: bool,
    pub(crate) single: bool,
    pub(crate) scope_is_range: bool,
    pub(crate) explicit_tool: Option<ToolName>,
    pub(crate) explicit_model_spec: Option<&'a str>,
    pub(crate) primary_tool: ToolName,
    pub(crate) resolved_tier_name: Option<&'a str>,
    pub(crate) config: Option<&'a ProjectConfig>,
    pub(crate) global_config: &'a GlobalConfig,
}

pub(crate) struct MultiReviewerPool {
    pub(crate) reviewer_tools: Vec<ToolName>,
    pub(crate) tier_reviewer_specs: Vec<crate::run_helpers::TierToolResolution>,
}

fn collect_tier_reviewer_specs(
    resolved_tier_name: Option<&str>,
    config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
) -> Vec<crate::run_helpers::TierToolResolution> {
    resolved_tier_name
        .and_then(|tier_name| {
            config.map(|cfg| {
                let effective_selection = cfg
                    .review
                    .as_ref()
                    .map(|review| &review.tool)
                    .unwrap_or(&global_config.review.tool);
                crate::run_helpers::collect_available_tier_models(
                    tier_name,
                    cfg,
                    effective_selection.whitelist(),
                    &[],
                )
            })
        })
        .unwrap_or_default()
}

fn collect_unique_tier_tools(
    tier_reviewer_specs: &[crate::run_helpers::TierToolResolution],
) -> Vec<ToolName> {
    let mut tier_reviewer_tools = Vec::new();
    for resolution in tier_reviewer_specs {
        if !tier_reviewer_tools.contains(&resolution.tool) {
            tier_reviewer_tools.push(resolution.tool);
        }
    }
    tier_reviewer_tools
}

fn build_selected_tool_subset(
    primary_tool: ToolName,
    tier_reviewer_tools: &[ToolName],
    reviewers: usize,
) -> Vec<ToolName> {
    let mut selected = vec![primary_tool];
    for tool in tier_reviewer_tools {
        if !selected.contains(tool) {
            selected.push(*tool);
        }
    }
    selected.truncate(reviewers);
    selected
}

pub(crate) fn resolve_auto_reviewer_selection(
    request: &AutoReviewerRequest<'_>,
) -> Option<AutoReviewerSelection> {
    if request.requested_reviewers != 1
        || request.explicit_reviewer_count
        || request.single
        || !request.scope_is_range
        || request.explicit_tool.is_some()
        || request.explicit_model_spec.is_some()
    {
        return None;
    }

    let tier_reviewer_specs = collect_tier_reviewer_specs(
        request.resolved_tier_name,
        request.config,
        request.global_config,
    );
    let tier_reviewer_tools = collect_unique_tier_tools(&tier_reviewer_specs);
    let unique_pool = build_selected_tool_subset(
        request.primary_tool,
        &tier_reviewer_tools,
        MAX_AUTO_HETEROGENEOUS_REVIEWERS,
    );

    (unique_pool.len() >= 2).then_some(AutoReviewerSelection {
        reviewers: unique_pool.len(),
        selected_tools: unique_pool,
    })
}

pub(crate) fn resolve_effective_reviewer_count(request: &AutoReviewerRequest<'_>) -> usize {
    let auto_reviewer_selection = resolve_auto_reviewer_selection(request);
    if let Some(selection) = auto_reviewer_selection {
        let tool_list = selection
            .selected_tools
            .iter()
            .map(|tool| tool.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        info!(
            "Auto-selected {} heterogeneous reviewers from tier '{}': {}",
            selection.reviewers,
            request
                .resolved_tier_name
                .unwrap_or("no tier name resolved"),
            tool_list
        );
        selection.reviewers
    } else {
        request.requested_reviewers
    }
}

pub(crate) fn resolve_multi_reviewer_pool(
    reviewers: usize,
    explicit_tool: Option<ToolName>,
    primary_tool: ToolName,
    resolved_tier_name: Option<&str>,
    config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
) -> Result<MultiReviewerPool> {
    let tier_reviewer_specs =
        collect_tier_reviewer_specs(resolved_tier_name, config, global_config);
    let tier_reviewer_tools = collect_unique_tier_tools(&tier_reviewer_specs);

    if let Some(tier_name) = resolved_tier_name {
        let unique_reviewer_tools = validate_multi_reviewer_tier_pool(
            tier_name,
            reviewers,
            primary_tool,
            &tier_reviewer_tools,
        )?;
        if reviewers > unique_reviewer_tools {
            warn!(
                tier = tier_name,
                requested_reviewers = reviewers,
                unique_tools = unique_reviewer_tools,
                "Multi-reviewer tier pool will reuse tools because fewer unique tier reviewers are available than requested"
            );
        }
    }

    let reviewer_tools = build_reviewer_tools(
        explicit_tool,
        primary_tool,
        config,
        Some(global_config),
        resolved_tier_name.map(|_| tier_reviewer_tools.as_slice()),
        reviewers,
    );

    Ok(MultiReviewerPool {
        reviewer_tools,
        tier_reviewer_specs,
    })
}

#[cfg(test)]
mod tests {
    use super::{AutoReviewerRequest, resolve_auto_reviewer_selection};
    use crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV;
    use crate::test_env_lock::TEST_ENV_LOCK;
    use csa_config::config::TierConfig;
    use csa_config::{
        GlobalConfig, ProjectConfig, ProjectMeta, ResourcesConfig, TierStrategy, ToolConfig,
    };
    use csa_core::types::ToolName;
    use std::collections::HashMap;

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

    fn assume_review_tools_available() -> (std::sync::MutexGuard<'static, ()>, ScopedEnvVarRestore)
    {
        (
            TEST_ENV_LOCK.lock().expect("review env lock poisoned"),
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
            session: Default::default(),
            memory: Default::default(),
            hooks: Default::default(),
            execution: Default::default(),
            vcs: Default::default(),
            filesystem_sandbox: Default::default(),
        }
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
            explicit_tool: None,
            explicit_model_spec: None,
            primary_tool: ToolName::Codex,
            resolved_tier_name: Some("quality"),
            config: Some(&config),
            global_config: &global,
        });

        assert!(selection.is_none());
    }
}
