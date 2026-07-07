use std::path::Path;

use anyhow::{Context, Result};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{OutputFormat, ToolName};
use csa_executor::{Executor, TransportResult};
use csa_session::MetaSessionState;

use super::super::result_contract::enforce_result_toml_path_contract;
use super::session_exec_audit;
use super::session_exec_runtime::SessionCompletionPlan;
use super::session_exec_write_guard::apply_write_restriction_violations;
use crate::pipeline::SessionExecutionResult;

#[path = "pipeline_session_exec_completion_require_commit.rs"]
mod require_commit;

const REVIEW_FIX_FINDING_TASK_TYPE: &str = "review_fix_finding";

pub(super) struct CompletionInput<'a> {
    pub(super) executor: &'a Executor,
    pub(super) tool: &'a ToolName,
    pub(super) prompt: &'a str,
    pub(super) output_format: &'a OutputFormat,
    pub(super) task_type: Option<&'a str>,
    pub(super) readonly_project_root: bool,
    pub(super) project_root: &'a Path,
    pub(super) config: Option<&'a ProjectConfig>,
    pub(super) global_config: Option<&'a GlobalConfig>,
    pub(super) session_dir: &'a Path,
    pub(super) memory_project_key: Option<String>,
    pub(super) effective_prompt: String,
    pub(super) plan: SessionCompletionPlan,
    pub(super) transport_result: TransportResult,
}

pub(super) async fn complete_session_execution(
    input: CompletionInput<'_>,
    session: &mut MetaSessionState,
) -> Result<SessionExecutionResult> {
    let SessionCompletionPlan {
        merged_env,
        hooks_config,
        sessions_root,
        edit_guard,
        new_file_guard,
        result_file_cleared,
        execution_start_time,
        commit_guard_enabled,
        require_commit_on_mutation,
        hook_bypass_scan_enabled,
        is_git,
        inside_git_worktree,
        pre_run_workspace,
        pre_exec_snapshot,
        timeout_diagnostics,
        sa_mode,
    } = input.plan;
    let merged_env_ref = (!merged_env.is_empty()).then_some(&merged_env);
    let transport_result = input.transport_result;
    let provider_session_id =
        csa_executor::extract_session_id_from_transport(input.tool, &transport_result);
    let events_count = transport_result
        .metadata
        .total_events_count
        .max(transport_result.events.len()) as u64;
    let execute_events_observed = crate::run_cmd::execute_tool_calls_observed(
        &transport_result.metadata,
        &transport_result.events,
    );
    let mut executed_shell_commands = crate::run_cmd::extract_executed_shell_commands(
        &transport_result.metadata,
        &transport_result.events,
    );
    if transport_result.metadata.has_no_verify_commit
        && crate::run_cmd::detect_no_verify_commit_commands(&executed_shell_commands).is_empty()
    {
        executed_shell_commands.push("git commit --no-verify".to_string());
    }
    let transcript_artifacts = crate::pipeline_transcript::persist_if_enabled(
        input.config,
        input.session_dir,
        &transport_result,
    );
    let has_tool_calls = transport_result.metadata.has_tool_calls
        || transport_result.metadata.has_execute_tool_calls;
    let turn_count = transport_result.metadata.turn_count;
    let output_tokens = transport_result.metadata.output_tokens;
    let mut result = transport_result.execution;
    crate::pipeline_sandbox::check_sandbox_permission_errors(
        &result.stderr_output,
        session.sandbox_info.as_ref(),
    );
    enforce_result_toml_path_contract(
        input.prompt,
        &input.effective_prompt,
        input.session_dir,
        session.turn_count,
        result_file_cleared,
        &mut result,
    );
    apply_write_restriction_violations(edit_guard, new_file_guard, input.executor, &mut result)?;
    if result.exit_code != 0 {
        crate::error_hints::append_sandbox_fs_denial_hint(
            &mut result.stderr_output,
            &result.output,
            crate::pipeline_sandbox::filesystem_sandbox_active(session.sandbox_info.as_ref()),
            &session.meta_session_id,
        );
    }
    let mut post_run_workspace = session_exec_audit::capture_git_workspace_snapshot_if_needed(
        is_git,
        input.project_root,
        require_commit_on_mutation,
    );
    let mut rescued_changed_paths = None;
    let mut commit_created = None;
    if commit_guard_enabled {
        let is_user_interrupted = result.exit_signal == Some(libc::SIGINT);
        let is_timed_out = result.terminal_reason.as_deref() == Some("timeout");
        let effective_require_commit_on_mutation = require_commit_on_mutation
            && !is_fix_finding_session(session)
            && !is_user_interrupted
            && !is_timed_out;
        commit_created = pre_run_workspace
            .as_ref()
            .zip(post_run_workspace.as_ref())
            .map(|(before, after)| before.head != after.head);
        let mut commit_guard = crate::run_cmd::evaluate_post_run_commit_guard(
            pre_run_workspace.as_ref(),
            post_run_workspace.as_ref(),
        );
        let mut policy_evaluation_failed = effective_require_commit_on_mutation
            && (!inside_git_worktree
                || pre_run_workspace.is_none()
                || post_run_workspace.is_none());
        if require_commit::should_attempt_require_commit_rescue(
            effective_require_commit_on_mutation,
            commit_guard.as_ref(),
        ) && let Some(new_head) =
            crate::run_cmd::attempt_rescue_commit(input.project_root, input.executor.tool_name())
        {
            commit_created = Some(true);
            rescued_changed_paths = Some(require_commit::compute_changed_paths_from_snapshots(
                pre_run_workspace.as_ref(),
                post_run_workspace.as_ref(),
            ));
            require_commit::record_require_commit_rescue(
                input.output_format,
                &mut result,
                input.executor.tool_name(),
                &new_head,
            );
            post_run_workspace = session_exec_audit::capture_git_workspace_snapshot_if_needed(
                is_git,
                input.project_root,
                require_commit_on_mutation,
            );
            commit_guard = crate::run_cmd::evaluate_post_run_commit_guard(
                pre_run_workspace.as_ref(),
                post_run_workspace.as_ref(),
            );
            policy_evaluation_failed = effective_require_commit_on_mutation
                && (!inside_git_worktree
                    || pre_run_workspace.is_none()
                    || post_run_workspace.is_none());
        }
        let git_commit_attempted =
            !crate::run_cmd::detect_git_commit_commands(&executed_shell_commands).is_empty();
        let commit_reflog_race = if git_commit_attempted && commit_created == Some(true) {
            let current_head = post_run_workspace
                .as_ref()
                .and_then(|snap| snap.head.as_deref());
            crate::run_cmd::detect_external_checkout_after_commit(
                input.project_root,
                current_head,
                execution_start_time,
            )
        } else {
            None
        };
        crate::run_cmd::apply_post_session_commit_policies(
            &mut result,
            crate::run_cmd::PostSessionCommitPolicyArgs {
                output_format: input.output_format,
                prompt: input.prompt,
                tool_name: input.executor.tool_name(),
                require_commit_on_mutation: effective_require_commit_on_mutation,
                commit_guard: commit_guard.as_ref(),
                policy_evaluation_failed,
                hook_bypass_scan_enabled,
                executed_shell_commands: &executed_shell_commands,
                commit_reflog_race: commit_reflog_race.as_ref(),
                merged_env_ref,
                execute_events_observed,
            },
        );
        apply_fix_finding_terminal_guard(
            session,
            commit_created,
            commit_guard.as_ref(),
            &mut result,
        );
        apply_fix_finding_terminal_guard_summary(input.project_root, session, &mut result);
    }
    let mut changed_paths = require_commit::compute_changed_paths_from_snapshots(
        pre_run_workspace.as_ref(),
        post_run_workspace.as_ref(),
    );
    if changed_paths.is_empty()
        && let Some(paths) = rescued_changed_paths
    {
        changed_paths = paths;
    }
    let snapshots_available = pre_run_workspace.is_some() && post_run_workspace.is_some();
    let post_ctx = crate::pipeline_post_exec::PostExecContext {
        executor: input.executor,
        prompt: input.prompt,
        effective_prompt: &input.effective_prompt,
        task_type: input.task_type,
        readonly_project_root: input.readonly_project_root,
        project_root: input.project_root,
        config: input.config,
        global_config: input.global_config,
        session_dir: input.session_dir.to_path_buf(),
        sessions_root,
        execution_start_time,
        hooks_config: &hooks_config,
        memory_project_key: input.memory_project_key,
        provider_session_id: provider_session_id.clone(),
        events_count,
        transcript_artifacts,
        changed_paths: changed_paths.clone(),
        pre_exec_snapshot,
        timeout_diagnostics,
        has_tool_calls,
        turn_count,
        output_tokens,
        sa_mode,
    };
    if let Err(err) =
        crate::pipeline_post_exec::process_execution_result(post_ctx, session, &mut result).await
    {
        crate::pipeline_post_exec::ensure_terminal_result_on_post_exec_error(
            input.project_root,
            session,
            input.executor.tool_name(),
            execution_start_time,
            &err,
        );
        return Err(err).with_context(|| format!("meta_session_id={}", session.meta_session_id));
    }
    Ok(SessionExecutionResult {
        execution: result,
        meta_session_id: session.meta_session_id.clone(),
        provider_session_id,
        changed_paths: snapshots_available.then_some(changed_paths),
        commit_created,
    })
}

fn apply_fix_finding_terminal_guard_summary(
    project_root: &Path,
    session: &MetaSessionState,
    result: &mut csa_process::ExecutionResult,
) {
    if !is_fix_finding_session(session) || !is_fix_finding_terminal_guard_failure(result) {
        return;
    }

    let reason = result
        .csa_gate_failure
        .as_deref()
        .unwrap_or("fix-finding-terminal-guard");
    let side_effects =
        crate::session_fix_finding_recovery::side_effect_diagnostic(project_root, session);
    let failure_detail = fix_finding_terminal_failure_detail(reason);
    let summary = format!(
        "`csa review --fix-finding` session failed closed: {failure_detail} \
         (reason={reason}). {side_effects}. Recovery: inspect `git status --short`, `git diff`, and \
         `git diff --staged`; preserve/finish or discard dirty side effects; create a \
         hook-enabled amend/commit if appropriate; then run a fresh exact-head \
         `csa review` before push/PR."
    );
    result.summary = summary.clone();
    if !result.stderr_output.contains(&summary) {
        if !result.stderr_output.is_empty() && !result.stderr_output.ends_with('\n') {
            result.stderr_output.push('\n');
        }
        result.stderr_output.push_str(&summary);
        result.stderr_output.push('\n');
    }
}

fn is_fix_finding_terminal_guard_failure(result: &csa_process::ExecutionResult) -> bool {
    result.csa_gate_failure.as_deref().is_some_and(|reason| {
        reason == "fix-finding-no-change"
            || crate::run_cmd::is_post_run_commit_policy_gate_failure(result)
    })
}

fn fix_finding_terminal_failure_detail(reason: &str) -> &'static str {
    match reason {
        "fix-finding-no-change" => "child reported success but no repository changes were detected",
        "commit-policy-ref-update" => "git commit was attempted but HEAD did not advance cleanly",
        "commit-policy-no-verify" => "forbidden git commit --no-verify was detected",
        "commit-policy-lefthook-bypass" => "forbidden LEFTHOOK bypass was detected during the fix",
        "commit-policy-unverifiable" => "CSA could not verify the repository mutation state",
        "commit-policy-uncommitted" => {
            "a strict commit policy required committed changes but dirty work remained"
        }
        _ => "a terminal fix-finding policy failed",
    }
}

fn apply_fix_finding_terminal_guard(
    session: &MetaSessionState,
    commit_created: Option<bool>,
    commit_guard: Option<&crate::run_cmd::PostRunCommitGuard>,
    result: &mut csa_process::ExecutionResult,
) {
    if !is_fix_finding_session(session)
        || result.exit_code != 0
        || crate::run_cmd::is_post_run_commit_policy_gate_failure(result)
    {
        return;
    }

    if commit_guard.is_some_and(|guard| guard.workspace_mutated) {
        return;
    }

    if commit_created == Some(false) {
        result.note_gate_failure("fix-finding-no-change");
    }
}

fn is_fix_finding_session(session: &MetaSessionState) -> bool {
    session.task_context.task_type.as_deref() == Some(REVIEW_FIX_FINDING_TASK_TYPE)
}

#[cfg(test)]
#[path = "pipeline_session_exec_completion_fix_finding_tests.rs"]
mod fix_finding_tests;

#[cfg(test)]
#[path = "pipeline_session_exec_completion_require_commit_tests.rs"]
mod require_commit_tests;

#[cfg(test)]
mod tests {
    use std::process::Command;

    use csa_core::transport_events::StreamingMetadata;
    use csa_core::types::OutputFormat;
    use csa_executor::{CodexRuntimeMetadata, TransportResult};
    use csa_session::{create_session, get_session_dir, load_result};

    use super::*;
    use crate::test_session_sandbox::ScopedSessionSandbox;

    fn run_git(project_root: &std::path::Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {} failed\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_capture(project_root: &std::path::Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {} failed\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("git output should be utf8")
            .trim()
            .to_string()
    }

    fn init_git_repo(project_root: &std::path::Path) {
        run_git(project_root, &["init", "-q"]);
        run_git(
            project_root,
            &["config", "user.email", "csa-test@example.com"],
        );
        run_git(project_root, &["config", "user.name", "CSA Test"]);
        run_git(project_root, &["config", "commit.gpgsign", "false"]);
        std::fs::write(project_root.join(".gitignore"), "state/\n").expect("write gitignore");
        std::fs::write(project_root.join("tracked.txt"), "initial\n").expect("write tracked");
        run_git(project_root, &["add", ".gitignore", "tracked.txt"]);
        run_git(project_root, &["commit", "-q", "-m", "initial"]);
    }

    #[tokio::test]
    async fn completion_warns_when_commit_reflog_is_followed_by_external_checkout() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _sandbox = ScopedSessionSandbox::new(&tmp).await;
        let project_root = tmp.path();
        init_git_repo(project_root);
        let primary_branch = git_capture(project_root, &["branch", "--show-current"]);

        run_git(project_root, &["checkout", "-q", "-b", "other"]);
        std::fs::write(project_root.join("tracked.txt"), "other\n").expect("write other");
        run_git(project_root, &["commit", "-am", "other"]);
        run_git(project_root, &["checkout", "-q", &primary_branch]);

        let before =
            crate::run_cmd::capture_git_workspace_snapshot(project_root, false).expect("snapshot");
        let execution_start_time = chrono::Utc::now() - chrono::Duration::seconds(1);
        std::fs::write(project_root.join("tracked.txt"), "child\n").expect("write child");
        run_git(project_root, &["commit", "-am", "child commit"]);
        run_git(project_root, &["checkout", "-q", "other"]);

        let mut session = create_session(
            project_root,
            Some("external checkout after commit"),
            None,
            Some("codex"),
        )
        .expect("create session");
        let session_dir =
            get_session_dir(project_root, &session.meta_session_id).expect("session dir");
        let executor = Executor::Codex {
            model_override: None,
            thinking_budget: None,
            runtime_metadata: CodexRuntimeMetadata::current(),
        };
        let transport_result = TransportResult {
            execution: csa_process::ExecutionResult {
                output: String::new(),
                stderr_output: "nothing to commit, working tree clean".to_string(),
                summary: "nothing to commit".to_string(),
                exit_code: 1,
                model_completed: Some(true),
                ..Default::default()
            },
            provider_session_id: None,
            events: Vec::new(),
            metadata: StreamingMetadata {
                extracted_commands: vec!["git commit -m 'child commit'".to_string()],
                has_tool_calls: true,
                has_execute_tool_calls: true,
                turn_count: 1,
                ..Default::default()
            },
        };
        let plan = SessionCompletionPlan {
            merged_env: Default::default(),
            hooks_config: Default::default(),
            sessions_root: session_dir
                .parent()
                .expect("sessions root")
                .display()
                .to_string(),
            edit_guard: None,
            new_file_guard: None,
            result_file_cleared: false,
            execution_start_time,
            commit_guard_enabled: true,
            require_commit_on_mutation: true,
            hook_bypass_scan_enabled: true,
            is_git: true,
            inside_git_worktree: true,
            pre_run_workspace: Some(before),
            pre_exec_snapshot: None,
            timeout_diagnostics: None,
            sa_mode: false,
        };

        let completed = complete_session_execution(
            CompletionInput {
                executor: &executor,
                tool: &csa_core::types::ToolName::Codex,
                prompt: "Commit the work",
                output_format: &OutputFormat::Json,
                task_type: Some("run"),
                readonly_project_root: false,
                project_root,
                config: None,
                global_config: None,
                session_dir: &session_dir,
                memory_project_key: None,
                effective_prompt: "Commit the work".to_string(),
                plan,
                transport_result,
            },
            &mut session,
        )
        .await
        .expect("complete session");

        assert_eq!(completed.commit_created, Some(true));
        assert!(completed.execution.csa_gate_failure.is_none());
        assert!(
            completed.execution.stderr_output.contains(
                "external checkout/reset moved the worktree before session completion (#2570)"
            ),
            "{}",
            completed.execution.stderr_output
        );
    }

    #[tokio::test]
    async fn completion_fails_when_git_commit_attempt_leaves_head_unchanged_and_staged() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _sandbox = ScopedSessionSandbox::new(&tmp).await;
        let project_root = tmp.path();
        init_git_repo(project_root);
        let before =
            crate::run_cmd::capture_git_workspace_snapshot(project_root, false).expect("snapshot");

        std::fs::write(project_root.join("tracked.txt"), "changed\n").expect("write change");
        run_git(project_root, &["add", "tracked.txt"]);

        let mut session =
            create_session(project_root, Some("commit ref update"), None, Some("codex"))
                .expect("create session");
        let session_dir =
            get_session_dir(project_root, &session.meta_session_id).expect("session dir");
        let executor = Executor::Codex {
            model_override: None,
            thinking_budget: None,
            runtime_metadata: CodexRuntimeMetadata::current(),
        };
        let transport_result = TransportResult {
            execution: csa_process::ExecutionResult {
                output: "git commit reported an object id before ref update failed".to_string(),
                stderr_output: String::new(),
                summary: "created commit object 49a0bad".to_string(),
                exit_code: 0,
                model_completed: Some(true),
                ..Default::default()
            },
            provider_session_id: None,
            events: Vec::new(),
            metadata: StreamingMetadata {
                extracted_commands: vec!["git commit -m fix".to_string()],
                has_tool_calls: true,
                has_execute_tool_calls: true,
                turn_count: 1,
                ..Default::default()
            },
        };
        let plan = SessionCompletionPlan {
            merged_env: Default::default(),
            hooks_config: Default::default(),
            sessions_root: session_dir
                .parent()
                .expect("sessions root")
                .display()
                .to_string(),
            edit_guard: None,
            new_file_guard: None,
            result_file_cleared: false,
            execution_start_time: chrono::Utc::now() - chrono::Duration::seconds(1),
            commit_guard_enabled: true,
            require_commit_on_mutation: false,
            hook_bypass_scan_enabled: true,
            is_git: true,
            inside_git_worktree: true,
            pre_run_workspace: Some(before),
            pre_exec_snapshot: None,
            timeout_diagnostics: None,
            sa_mode: false,
        };

        let completed = complete_session_execution(
            CompletionInput {
                executor: &executor,
                tool: &csa_core::types::ToolName::Codex,
                prompt: "Fix, verify, and commit the work",
                output_format: &OutputFormat::Json,
                task_type: Some("run"),
                readonly_project_root: false,
                project_root,
                config: None,
                global_config: None,
                session_dir: &session_dir,
                memory_project_key: None,
                effective_prompt: "Fix, verify, and commit the work".to_string(),
                plan,
                transport_result,
            },
            &mut session,
        )
        .await
        .expect("complete session");

        assert_eq!(completed.execution.exit_code, 1);
        assert_eq!(
            completed.execution.csa_gate_failure.as_deref(),
            Some("commit-policy-ref-update")
        );
        let persisted = load_result(project_root, &session.meta_session_id)
            .expect("load result")
            .expect("result should be saved");
        assert_eq!(persisted.status, "failure");
        assert_eq!(persisted.exit_code, 1);
    }

    #[tokio::test]
    async fn completion_reports_commit_created_when_head_advances_cleanly() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _sandbox = ScopedSessionSandbox::new(&tmp).await;
        let project_root = tmp.path();
        init_git_repo(project_root);
        let before =
            crate::run_cmd::capture_git_workspace_snapshot(project_root, false).expect("snapshot");

        std::fs::write(project_root.join("tracked.txt"), "committed\n").expect("write change");
        run_git(project_root, &["add", "tracked.txt"]);
        run_git(project_root, &["commit", "-q", "-m", "clean commit"]);

        let mut session = create_session(project_root, Some("clean commit"), None, Some("codex"))
            .expect("create session");
        let session_dir =
            get_session_dir(project_root, &session.meta_session_id).expect("session dir");
        let executor = Executor::Codex {
            model_override: None,
            thinking_budget: None,
            runtime_metadata: CodexRuntimeMetadata::current(),
        };
        let transport_result = TransportResult {
            execution: csa_process::ExecutionResult {
                output: "committed".to_string(),
                stderr_output: String::new(),
                summary: "committed".to_string(),
                exit_code: 0,
                model_completed: Some(true),
                ..Default::default()
            },
            provider_session_id: None,
            events: Vec::new(),
            metadata: StreamingMetadata {
                has_tool_calls: true,
                has_execute_tool_calls: true,
                turn_count: 1,
                ..Default::default()
            },
        };
        let plan = SessionCompletionPlan {
            merged_env: Default::default(),
            hooks_config: Default::default(),
            sessions_root: session_dir
                .parent()
                .expect("sessions root")
                .display()
                .to_string(),
            edit_guard: None,
            new_file_guard: None,
            result_file_cleared: false,
            execution_start_time: chrono::Utc::now() - chrono::Duration::seconds(1),
            commit_guard_enabled: true,
            require_commit_on_mutation: true,
            hook_bypass_scan_enabled: true,
            is_git: true,
            inside_git_worktree: true,
            pre_run_workspace: Some(before),
            pre_exec_snapshot: None,
            timeout_diagnostics: None,
            sa_mode: false,
        };

        let completed = complete_session_execution(
            CompletionInput {
                executor: &executor,
                tool: &csa_core::types::ToolName::Codex,
                prompt: "Commit the work",
                output_format: &OutputFormat::Json,
                task_type: Some("run"),
                readonly_project_root: false,
                project_root,
                config: None,
                global_config: None,
                session_dir: &session_dir,
                memory_project_key: None,
                effective_prompt: "Commit the work".to_string(),
                plan,
                transport_result,
            },
            &mut session,
        )
        .await
        .expect("complete session");

        assert_eq!(completed.commit_created, Some(true));
        assert_eq!(completed.execution.exit_code, 0);
        assert!(
            !completed
                .execution
                .stderr_output
                .contains("CSA require-commit rescue"),
            "{}",
            completed.execution.stderr_output
        );
    }

    #[test]
    fn fix_finding_terminal_guard_allows_dirty_side_effects_after_amend() {
        let mut session = MetaSessionState::default();
        session.task_context.task_type = Some(REVIEW_FIX_FINDING_TASK_TYPE.to_string());
        let commit_guard = crate::run_cmd::PostRunCommitGuard {
            workspace_mutated: true,
            head_changed: true,
            head_externally_raced: false,
            changed_paths: vec!["tracked.txt".to_string()],
        };
        let mut result = csa_process::ExecutionResult {
            exit_code: 0,
            model_completed: Some(true),
            ..Default::default()
        };

        apply_fix_finding_terminal_guard(&session, Some(true), Some(&commit_guard), &mut result);

        assert_eq!(result.exit_code, 0);
        assert!(result.csa_gate_failure.is_none());
    }
}
