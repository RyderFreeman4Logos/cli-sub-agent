#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_budget_exhaustion_stops_retry_before_runtime_fallback() {
        let project_root = tempfile::tempdir().expect("project root");
        let mut result = csa_process::ExecutionResult {
            exit_code: 1,
            summary: "failed".to_string(),
            ..Default::default()
        };
        let mut global_config = GlobalConfig::default();
        global_config.budget.max_tokens_per_issue = 10;

        let mut tried_tools = Vec::new();
        let mut tried_specs = Vec::new();
        let mut runtime_fallback_candidates = vec![ToolName::ClaudeCode];
        let mut runtime_fallback_attempts = 0;
        let mut fallback_chain: csa_scheduler::FallbackChain = Vec::new();
        let mut accumulated_changed_paths = Vec::new();
        let mut all_attempt_change_snapshots_available = true;
        let mut pre_created_fork_session_id = None;

        let model_catalog = csa_config::EffectiveModelCatalog::shipped()
            .expect("shipped model catalog must be valid");
        let action = evaluate_post_attempt_retry(
            PostAttemptRequest {
                exec_result: &mut result,
                exec_changed_paths: None,
                issue_tokens_used: 10,
                runtime_fallback_enabled: true,
                max_runtime_fallback_attempts: 3,
                current_tool: ToolName::Codex,
                tool_name: ToolName::Codex.as_str(),
                current_model_spec: None,
                attempts: 1,
                max_failover_attempts: 3,
                tier_auto_select: false,
                resolved_tier_name: None,
                tier_failover_tool_filter: None,
                executed_session_id: None,
                effective_session_arg: None,
                ephemeral: false,
                prompt_text: "do work",
                project_root: project_root.path(),
                config: None,
                global_config: &global_config,
                model_catalog: &model_catalog,
                task_needs_edit: None,
                attempt_elapsed: Duration::ZERO,
            },
            PostAttemptState {
                tried_tools: &mut tried_tools,
                tried_specs: &mut tried_specs,
                runtime_fallback_candidates: &mut runtime_fallback_candidates,
                runtime_fallback_attempts: &mut runtime_fallback_attempts,
                fallback_chain: &mut fallback_chain,
                accumulated_changed_paths: &mut accumulated_changed_paths,
                all_attempt_change_snapshots_available: &mut all_attempt_change_snapshots_available,
                pre_created_fork_session_id: &mut pre_created_fork_session_id,
            },
        )
        .expect("post-attempt evaluation should succeed");

        assert!(matches!(action, PostAttemptAction::Break(None)));
        assert_eq!(runtime_fallback_attempts, 0);
        assert!(tried_tools.is_empty());
        assert!(
            result.warnings.iter().any(
                |warning| warning.contains("Issue token budget exhausted; stopping retry loop")
            )
        );
        assert!(
            result
                .stderr_output
                .contains("Issue token budget exhausted; stopping retry loop (used=10, max=10).")
        );
    }
}
