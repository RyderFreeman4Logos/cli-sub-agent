use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;
use csa_scheduler::RateLimitDetected;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TierFilter {
    All,
    Whitelist(Vec<String>),
}

impl TierFilter {
    pub(crate) fn all() -> Self {
        Self::All
    }

    pub(crate) fn whitelist<I, S>(tools: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::Whitelist(tools.into_iter().map(Into::into).collect())
    }

    fn whitelist_slice(&self) -> Option<&[String]> {
        match self {
            Self::All => None,
            Self::Whitelist(tools) => Some(tools.as_slice()),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TierAttemptFailure {
    pub(crate) model_spec: String,
    pub(crate) reason: String,
}

pub(crate) fn ordered_tier_candidates(
    initial_tool: ToolName,
    initial_model_spec: Option<&str>,
    tier_name: Option<&str>,
    config: Option<&ProjectConfig>,
    global_config: Option<&GlobalConfig>,
    tier_fallback_enabled: bool,
    tier_filter: Option<&TierFilter>,
) -> Vec<(ToolName, Option<String>)> {
    if !tier_fallback_enabled {
        return vec![(initial_tool, initial_model_spec.map(str::to_string))];
    }

    let Some(tier_name) = tier_name else {
        return ordered_global_candidates(
            initial_tool,
            initial_model_spec,
            config,
            global_config,
            tier_filter,
        );
    };
    let Some(cfg) = config else {
        return vec![(initial_tool, initial_model_spec.map(str::to_string))];
    };

    let mut ordered = Vec::new();
    if let Some(spec) = initial_model_spec {
        ordered.push((initial_tool, Some(spec.to_string())));
    }

    for resolution in crate::run_helpers::collect_available_tier_models(
        tier_name,
        cfg,
        tier_filter.and_then(TierFilter::whitelist_slice),
        &[],
    ) {
        if ordered.iter().any(|(_, existing_spec)| {
            existing_spec.as_deref() == Some(resolution.model_spec.as_str())
        }) {
            continue;
        }
        ordered.push((resolution.tool, Some(resolution.model_spec)));
    }

    if ordered.is_empty() {
        ordered.push((initial_tool, initial_model_spec.map(str::to_string)));
    }

    ordered
}

fn ordered_global_candidates(
    initial_tool: ToolName,
    initial_model_spec: Option<&str>,
    config: Option<&ProjectConfig>,
    global_config: Option<&GlobalConfig>,
    tier_filter: Option<&TierFilter>,
) -> Vec<(ToolName, Option<String>)> {
    let mut ordered = vec![(initial_tool, initial_model_spec.map(str::to_string))];
    let Some(global_config) = global_config else {
        return ordered;
    };

    for tool in csa_config::global::sort_tools_by_effective_priority(
        csa_config::global::all_known_tools(),
        config,
        global_config,
    ) {
        if tool == initial_tool {
            continue;
        }
        if let Some(whitelist) = tier_filter.and_then(TierFilter::whitelist_slice)
            && !whitelist.iter().any(|candidate| candidate == tool.as_str())
        {
            continue;
        }
        if config.is_some_and(|cfg| !cfg.is_tool_auto_selectable(tool.as_str())) {
            continue;
        }
        if !crate::run_helpers::is_tool_binary_available_for_config(tool.as_str(), config) {
            continue;
        }
        ordered.push((tool, None));
    }

    ordered
}

pub(crate) fn classify_next_model_failure(
    tool_name: &str,
    stderr: &str,
    stdout: &str,
    exit_code: i32,
    model_spec: Option<&str>,
) -> Option<RateLimitDetected> {
    csa_scheduler::detect_rate_limit(tool_name, stderr, stdout, exit_code, model_spec)
        .filter(|detected| detected.advance_to_next_model)
}

pub(crate) fn chain_failure_reasons(failures: &[TierAttemptFailure]) -> Option<String> {
    (!failures.is_empty()).then(|| {
        failures
            .iter()
            .map(|failure| failure.reason.as_str())
            .collect::<Vec<_>>()
            .join("; ")
    })
}

pub(crate) fn format_all_models_failed_reason(
    tier_name: Option<&str>,
    failures: &[TierAttemptFailure],
) -> Option<String> {
    (!failures.is_empty()).then(|| {
        let tier_label = tier_name.unwrap_or("tier");
        let details = failures
            .iter()
            .map(|failure| format!("{}={}", failure.model_spec, failure.reason))
            .collect::<Vec<_>>()
            .join(", ");
        format!("all {tier_label} models failed: {details}")
    })
}

pub(crate) fn fallback_reason_for_result(failures: &[TierAttemptFailure]) -> Option<&'static str> {
    failures
        .iter()
        .any(|failure| {
            let reason = failure.reason.to_ascii_lowercase();
            reason.contains("429") || reason.contains("quota")
        })
        .then_some("429_quota_exhausted")
}

pub(crate) fn persist_fallback_result_fields(
    project_root: &Path,
    session_id: &str,
    original_tool: ToolName,
    fallback_tool: ToolName,
    fallback_reason: Option<&str>,
) {
    let Some(reason) = fallback_reason else {
        return;
    };
    let Ok(Some(mut result)) = csa_session::load_result(project_root, session_id) else {
        return;
    };
    result.original_tool = Some(original_tool.as_str().to_string());
    result.fallback_tool = Some(fallback_tool.as_str().to_string());
    result.fallback_reason = Some(reason.to_string());
    if let Err(err) = csa_session::save_result(project_root, session_id, &result) {
        tracing::warn!(
            session_id,
            error = %err,
            "Failed to persist runtime fallback fields in result.toml"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{TierFilter, ordered_tier_candidates};
    use csa_config::{GlobalConfig, ProjectConfig, ToolConfig};
    use csa_core::types::ToolName;
    use std::collections::HashMap;

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

    #[test]
    fn tier_fallback_respects_original_tool_whitelist() {
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
            Some(&TierFilter::whitelist(["codex"])),
        );

        assert_eq!(
            candidates,
            vec![(
                ToolName::Codex,
                Some("codex/openai/gpt-5.4/high".to_string())
            )]
        );
    }

    #[test]
    fn no_tier_fallback_uses_global_tool_priority() {
        let _availability = crate::test_env_lock::ScopedEnvVarRestore::set(
            crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV,
            "1",
        );
        let mut global = GlobalConfig::default();
        global.preferences.tool_priority =
            vec!["claude-code".to_string(), "gemini-cli".to_string()];

        let candidates = ordered_tier_candidates(
            ToolName::Codex,
            None,
            None,
            None,
            Some(&global),
            true,
            Some(&TierFilter::whitelist(["gemini-cli", "claude-code"])),
        );

        assert_eq!(
            candidates,
            vec![
                (ToolName::Codex, None),
                (ToolName::ClaudeCode, None),
                (ToolName::GeminiCli, None),
            ]
        );
    }
}
