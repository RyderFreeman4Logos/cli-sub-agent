use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use csa_config::ProjectConfig;
use tokio::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PostExecGateCommandOutcome {
    Exited(Option<i32>),
    TimedOut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PostExecGateOutcome {
    Passed,
    Skipped,
}

type PostExecGateFuture = Pin<Box<dyn Future<Output = Result<PostExecGateCommandOutcome>> + Send>>;

fn is_post_exec_gate_exempt_prompt(prompt_text: &str) -> bool {
    let prompt = prompt_text.trim_start();
    prompt.starts_with("# REVIEW:") || prompt.starts_with("# DEBATE:")
}

fn post_exec_gate_requires_changes(
    project_root: &Path,
    skip_on_no_changes: bool,
    changed_paths: Option<&[String]>,
) -> Result<bool> {
    if !skip_on_no_changes || !crate::run_cmd::is_git_worktree(project_root) {
        return Ok(true);
    }

    if let Some(paths) = changed_paths {
        if paths.is_empty() {
            return Ok(false);
        }
        return git_diff_has_changes(project_root);
    }

    git_diff_has_changes(project_root)
}

fn git_diff_has_changes(project_root: &Path) -> Result<bool> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["diff", "--quiet", "HEAD", "--"])
        .output()
        .with_context(|| {
            format!(
                "failed to inspect git diff for post-exec gate in {}",
                project_root.display()
            )
        })?;

    match output.status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Ok(true),
    }
}

pub(super) fn execute_post_exec_gate_command(
    command: &str,
    project_root: &Path,
    timeout_seconds: u64,
) -> PostExecGateFuture {
    let command = command.to_string();
    let project_root = project_root.to_path_buf();

    Box::pin(async move {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(&command)
            .current_dir(&project_root)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        #[cfg(unix)]
        {
            cmd.process_group(0);
        }

        let mut child = cmd.spawn().with_context(|| {
            format!(
                "failed to spawn post-exec gate command `{command}` in {}",
                project_root.display()
            )
        })?;
        let child_pid = child.id();

        match tokio::time::timeout(Duration::from_secs(timeout_seconds), child.wait()).await {
            Ok(wait_result) => {
                let status = wait_result.with_context(|| {
                    format!(
                        "failed while waiting for post-exec gate command `{command}` in {}",
                        project_root.display()
                    )
                })?;
                Ok(PostExecGateCommandOutcome::Exited(status.code()))
            }
            Err(_) => {
                #[cfg(unix)]
                {
                    if let Some(pid) = child_pid {
                        // SAFETY: kill() is async-signal-safe. Negative PID targets the process group.
                        unsafe {
                            libc::kill(-(pid as i32), libc::SIGKILL);
                        }
                    } else {
                        let _ = child.start_kill();
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = child.start_kill();
                }

                let _ = child.wait().await;
                Ok(PostExecGateCommandOutcome::TimedOut)
            }
        }
    })
}

pub(super) async fn maybe_run_post_exec_gate_with_runner<F>(
    project_root: &Path,
    prompt_text: &str,
    session_id: Option<&str>,
    config: Option<&ProjectConfig>,
    changed_paths: Option<&[String]>,
    runner: F,
) -> Result<PostExecGateOutcome>
where
    F: FnOnce(&str, &Path, u64) -> PostExecGateFuture,
{
    let gate_config = config
        .map(|cfg| cfg.run.post_exec_gate.clone())
        .unwrap_or_default();

    if !gate_config.enabled || is_post_exec_gate_exempt_prompt(prompt_text) {
        return Ok(PostExecGateOutcome::Skipped);
    }

    if !post_exec_gate_requires_changes(
        project_root,
        gate_config.skip_on_no_changes,
        changed_paths,
    )? {
        return Ok(PostExecGateOutcome::Skipped);
    }

    let branch = super::run_context::current_branch_name(project_root);
    match runner(
        &gate_config.command,
        project_root,
        gate_config.timeout_seconds,
    )
    .await?
    {
        PostExecGateCommandOutcome::Exited(Some(0)) => Ok(PostExecGateOutcome::Passed),
        PostExecGateCommandOutcome::Exited(code) => anyhow::bail!(
            "csa: post-exec gate failed (exit={}).\n\
             gate command: {}\n\
             cwd: {}\n\
             employee session: {}\n\
             branch: {}\n\
             next step: inspect the gate output above, fix the issue, and re-run the dispatch manually. v1 gate does NOT auto-retry.",
            code.map_or_else(|| "signal".to_string(), |value| value.to_string()),
            gate_config.command,
            project_root.display(),
            session_id.unwrap_or("(ephemeral)"),
            branch,
        ),
        PostExecGateCommandOutcome::TimedOut => anyhow::bail!(
            "csa: post-exec gate timed out after {} seconds.\n\
             gate command: {}\n\
             cwd: {}\n\
             employee session: {}\n\
             branch: {}\n\
             next step: inspect the gate output above, fix the issue, and re-run the dispatch manually. v1 gate does NOT auto-retry.",
            gate_config.timeout_seconds,
            gate_config.command,
            project_root.display(),
            session_id.unwrap_or("(ephemeral)"),
            branch,
        ),
    }
}

#[cfg(test)]
#[path = "run_cmd_execute_post_exec_tests.rs"]
mod post_exec_tests;
