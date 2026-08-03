use std::borrow::Cow;

use anyhow::Result;
use csa_session::state::MetaSessionState;

use super::{ExecuteOptions, Executor};

impl Executor {
    pub(crate) async fn apply_pre_session_hook<'a>(
        &self,
        prompt: &'a str,
        session: &MetaSessionState,
        options: &ExecuteOptions,
    ) -> Result<Cow<'a, str>> {
        let Some(invocation) = options.pre_session_hook.as_ref() else {
            return Ok(Cow::Borrowed(prompt));
        };
        let config = invocation.config();
        if !config.enabled {
            tracing::debug!("pre_session hook disabled");
            return Ok(Cow::Borrowed(prompt));
        }
        if !config.matches_transport(self.tool_name()) {
            tracing::debug!(
                transport = self.tool_name(),
                configured = ?config.transports,
                "pre_session hook skipped by transport filter"
            );
            return Ok(Cow::Borrowed(prompt));
        }
        if !invocation.claim_first_fire() {
            tracing::debug!("pre_session hook already fired for this invocation");
            return Ok(Cow::Borrowed(prompt));
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

        Ok(csa_hooks::run_pre_session_hook_with_cancellation(
            config,
            &context,
            options.cancellation.as_ref(),
        )
        .await?
        .map_or(Cow::Borrowed(prompt), Cow::Owned))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use csa_process::StreamMode;
    use csa_session::state::{
        ContextStatus, Genealogy, MetaSessionState, SessionPhase, TaskContext,
    };
    use std::{collections::HashMap, time::Duration};

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
            .await
            .expect("hook execution");

        // The hook output should contain /tmp (the session project_path),
        // not the process's current working directory.
        assert!(
            result.contains("/tmp"),
            "hook cwd should be session.project_path (/tmp), got: {result}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_during_pre_session_hook_terminates_group_before_transport_spawn() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let pid_file = temp.path().join("descendant.pid");
        let ready_file = temp.path().join("descendant.ready");
        let marker = temp.path().join("transport-started");
        let bin = temp.path().join("bin");
        std::fs::create_dir(&bin).expect("create bin dir");
        let transport = bin.join("opencode");
        std::fs::write(&transport, "#!/bin/sh\nprintf started > \"$MARKER\"\n")
            .expect("write fake transport");
        let mut permissions = std::fs::metadata(&transport)
            .expect("transport metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&transport, permissions).expect("make transport executable");

        let config = csa_hooks::PreSessionHookConfig {
            command: Some(format!(
                "sh -c 'trap \"\" TERM; echo $$ > \"{}\"; : > \"{}\"; sleep 5' >/dev/null 2>&1 & while [ ! -e \"{}\" ]; do sleep 0.01; done; sleep 5",
                pid_file.display(),
                ready_file.display(),
                ready_file.display()
            )),
            transports: vec!["opencode".to_string()],
            timeout_seconds: 10,
            ..Default::default()
        };
        let cancellation = csa_process::ExecutionCancellation::new();
        let invocation = csa_hooks::PreSessionHookInvocation::new(config);
        let mut options =
            ExecuteOptions::new(StreamMode::BufferOnly, 60).with_pre_session_hook(invocation);
        options.cancellation = Some(cancellation.clone());
        let executor = Executor::Opencode {
            model_override: None,
            agent: None,
            thinking_budget: None,
        };
        let mut session = test_session();
        session.project_path = temp.path().display().to_string();
        let extra_env = HashMap::from([
            ("PATH".to_string(), bin.display().to_string()),
            ("MARKER".to_string(), marker.display().to_string()),
        ]);
        let cancel_after_ready = async {
            while !ready_file.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let pid = std::fs::read_to_string(&pid_file)
                .expect("descendant publishes pid after TERM trap")
                .trim()
                .parse::<i32>()
                .expect("numeric descendant pid");
            cancellation.cancel();
            pid
        };

        let (result, pid) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(
                executor.execute_with_transport(
                    "hello",
                    None,
                    &session,
                    Some(&extra_env),
                    options,
                    None,
                ),
                cancel_after_ready
            )
        })
        .await
        .expect("cancellation must bound the full execution boundary");

        let error = result.expect_err("cancelled execution must fail");
        assert!(error.to_string().contains("cancelled"), "error={error:#}");
        assert!(
            !marker.exists(),
            "transport child must not start after cancellation"
        );
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        assert!(
            stat.is_empty() || stat.split_whitespace().nth(2) == Some("Z"),
            "hook descendant must be terminated before return; stat={stat}"
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
            .await
            .expect("first hook execution");
        let second = executor
            .apply_pre_session_hook("second prompt", &session, &second_options)
            .await
            .expect("second hook execution");

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
