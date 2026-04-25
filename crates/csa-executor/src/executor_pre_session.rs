use std::borrow::Cow;

use csa_session::state::MetaSessionState;

use super::{ExecuteOptions, Executor};

impl Executor {
    pub(crate) async fn apply_pre_session_hook<'a>(
        &self,
        prompt: &'a str,
        session: &MetaSessionState,
        options: &ExecuteOptions,
    ) -> Cow<'a, str> {
        let Some(invocation) = options.pre_session_hook.as_ref() else {
            return Cow::Borrowed(prompt);
        };
        let config = invocation.config();
        if !config.enabled {
            tracing::debug!("pre_session hook disabled");
            return Cow::Borrowed(prompt);
        }
        if !config.matches_transport(self.tool_name()) {
            tracing::debug!(
                transport = self.tool_name(),
                configured = ?config.transports,
                "pre_session hook skipped by transport filter"
            );
            return Cow::Borrowed(prompt);
        }
        if !invocation.claim_first_fire() {
            tracing::debug!("pre_session hook already fired for this invocation");
            return Cow::Borrowed(prompt);
        }
        let working_dir = if session.project_path.is_empty() {
            std::env::current_dir()
                .map(|path| path.display().to_string())
                .unwrap_or_default()
        } else {
            session.project_path.clone()
        };
        let context = csa_hooks::PreSessionHookContext {
            session_id: &session.meta_session_id,
            transport: self.tool_name(),
            project_root: &session.project_path,
            working_dir: &working_dir,
            user_prompt: prompt,
        };

        csa_hooks::run_pre_session_hook(config, &context)
            .await
            .map_or(Cow::Borrowed(prompt), Cow::Owned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use csa_process::StreamMode;
    use csa_session::state::{
        ContextStatus, Genealogy, MetaSessionState, SessionPhase, TaskContext,
    };
    use std::collections::HashMap;

    fn test_session() -> MetaSessionState {
        let now = chrono::Utc::now();
        MetaSessionState {
            meta_session_id: "01PRESESSION00000000000000".to_string(),
            description: Some("pre-session test".to_string()),
            project_path: "/tmp".to_string(),
            branch: None,
            created_at: now,
            last_accessed: now,
            csa_version: None,
            genealogy: Genealogy::default(),
            tools: HashMap::new(),
            context_status: ContextStatus::default(),
            total_token_usage: None,
            phase: SessionPhase::Active,
            task_context: TaskContext::default(),
            turn_count: 0,
            token_budget: None,
            sandbox_info: None,
            termination_reason: None,
            is_seed_candidate: false,
            git_head_at_creation: None,
            pre_session_porcelain: None,
            last_return_packet: None,
            change_id: None,
            spec_id: None,
            fork_call_timestamps: Vec::new(),
            vcs_identity: None,
            identity_version: 1,
        }
    }

    #[tokio::test]
    async fn pre_session_hook_uses_session_project_path_as_cwd() {
        // session.project_path should be preferred over std::env::current_dir()
        // when determining hook working directory.
        let config = csa_hooks::PreSessionHookConfig {
            command: Some("pwd".to_string()),
            transports: vec!["codex".to_string()],
            timeout_seconds: 2,
            ..Default::default()
        };
        let invocation = csa_hooks::PreSessionHookInvocation::new(config);
        let options =
            ExecuteOptions::new(StreamMode::BufferOnly, 60).with_pre_session_hook(invocation);
        let executor = Executor::Codex {
            model_override: None,
            thinking_budget: None,
            runtime_metadata: crate::codex_runtime::codex_runtime_metadata(),
        };
        let mut session = test_session();
        // Use /tmp which always exists and differs from process cwd
        session.project_path = "/tmp".to_string();

        let result = executor
            .apply_pre_session_hook("hello", &session, &options)
            .await;

        // The hook output should contain /tmp (the session project_path),
        // not the process's current working directory.
        assert!(
            result.contains("/tmp"),
            "hook cwd should be session.project_path (/tmp), got: {result}"
        );
    }

    #[tokio::test]
    async fn pre_session_hook_fires_once_across_cloned_execute_options() {
        let config = csa_hooks::PreSessionHookConfig {
            command: Some("printf 'hook context\\n'".to_string()),
            transports: vec!["codex".to_string()],
            timeout_seconds: 2,
            ..Default::default()
        };
        let invocation = csa_hooks::PreSessionHookInvocation::new(config);
        let options = ExecuteOptions::new(StreamMode::BufferOnly, 60)
            .with_pre_session_hook(invocation.clone());
        let second_options = options.clone();
        let executor = Executor::Codex {
            model_override: None,
            thinking_budget: None,
            runtime_metadata: crate::codex_runtime::codex_runtime_metadata(),
        };
        let session = test_session();

        let first = executor
            .apply_pre_session_hook("first prompt", &session, &options)
            .await;
        let second = executor
            .apply_pre_session_hook("second prompt", &session, &second_options)
            .await;

        assert!(
            first
                .starts_with("<system-reminder>\nhook context\n</system-reminder>\n\nfirst prompt"),
            "first prompt should receive hook context, got: {first}"
        );
        assert_eq!(
            second.as_ref(),
            "second prompt",
            "second turn in the same invocation must not receive hook context"
        );
    }
}
