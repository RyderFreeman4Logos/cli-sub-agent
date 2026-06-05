use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use csa_config::ProjectConfig;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::process::Command;
use tokio::task::JoinHandle;

/// Outcome of running the post-exec gate command, including the combined
/// stdout+stderr captured for structured failure surfacing (#1726). The output
/// is always tee'd to the parent's stdout/stderr too, so the raw transcript
/// (`full.md`) is unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PostExecGateCommandOutcome {
    pub(super) exit: PostExecGateCommandExit,
    pub(super) captured_output: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PostExecGateCommandExit {
    Exited(Option<i32>),
    TimedOut,
}

/// Test-only constructors. Production builds the struct literal directly (the
/// real runner threads an arbitrary exit AND captured output through at once),
/// so these convenience constructors are only used by the synthetic runners in
/// the test submodule.
#[cfg(test)]
impl PostExecGateCommandOutcome {
    /// Outcome with no captured output (test helper / synthetic runners).
    pub(super) fn exited(code: Option<i32>) -> Self {
        Self {
            exit: PostExecGateCommandExit::Exited(code),
            captured_output: String::new(),
        }
    }

    /// Outcome carrying captured gate output (used by the real runner and by
    /// tests that exercise the structured surfacing path).
    pub(super) fn exited_with(code: Option<i32>, output: impl Into<String>) -> Self {
        Self {
            exit: PostExecGateCommandExit::Exited(code),
            captured_output: output.into(),
        }
    }

    /// Timeout outcome with no captured output (test helper).
    pub(super) fn timed_out() -> Self {
        Self {
            exit: PostExecGateCommandExit::TimedOut,
            captured_output: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PostExecGateOutcome {
    Passed,
    Skipped,
    Failed(PostExecGateFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PostExecGateFailure {
    kind: PostExecGateFailureKind,
    diagnostic: String,
    /// The gate command that failed (e.g. `"just pre-commit"`).
    gate_command: String,
    /// Combined captured stdout+stderr of the gate command (pre-redaction).
    captured_output: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PostExecGateFailureKind {
    Exited(Option<i32>),
    TimedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PostExecGateWorktreeState {
    CommittedClean,
    DirtyOrUnknown,
}

impl PostExecGateFailure {
    fn into_error(self) -> anyhow::Error {
        anyhow::anyhow!(self.diagnostic)
    }

    fn is_timeout(&self) -> bool {
        matches!(self.kind, PostExecGateFailureKind::TimedOut)
    }

    /// Real exit code for the structured report: a signal-kill (no code) maps to
    /// `-1`; a timeout maps to `124` (conventional timeout exit code).
    fn report_exit_code(&self) -> i32 {
        match self.kind {
            PostExecGateFailureKind::Exited(Some(code)) => code,
            PostExecGateFailureKind::Exited(None) => -1,
            PostExecGateFailureKind::TimedOut => 124,
        }
    }
}

type PostExecGateFuture = Pin<Box<dyn Future<Output = Result<PostExecGateCommandOutcome>> + Send>>;

/// Read `reader` to EOF, re-emitting each chunk to the parent's `sink` (so the
/// raw transcript stays intact) while appending it to the shared `captured`
/// buffer for structured failure surfacing (#1726).
fn tee_gate_stream<R, W>(reader: R, sink: W, captured: Arc<Mutex<Vec<u8>>>) -> JoinHandle<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    tokio::spawn(async move {
        let mut reader = reader;
        let mut sink = sink;
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => {
                    let bytes = &chunk[..n];
                    let _ = sink.write_all(bytes).await;
                    let _ = sink.flush().await;
                    if let Ok(mut guard) = captured.lock() {
                        guard.extend_from_slice(bytes);
                    }
                }
                Err(_) => break,
            }
        }
    })
}

pub(super) struct PostExecGateApplyOptions<'a> {
    pub(super) changed_paths: Option<&'a [String]>,
    pub(super) extra_env: Option<HashMap<String, String>>,
    pub(super) no_post_exec_gate: bool,
    pub(super) planning_only: bool,
}

fn is_post_exec_gate_exempt_prompt(prompt_text: &str) -> bool {
    let prompt = prompt_text.trim_start();
    prompt.starts_with("# REVIEW:") || prompt.starts_with("# DEBATE:")
}

fn post_exec_gate_requires_changes(
    project_root: &Path,
    skip_on_no_changes: bool,
    session_id: Option<&str>,
    changed_paths: Option<&[String]>,
) -> Result<bool> {
    if !skip_on_no_changes || !crate::run_cmd::is_git_worktree(project_root) {
        return Ok(true);
    }

    let start_head = session_id.and_then(|id| session_start_head(project_root, id));
    if let Some(paths) = changed_paths {
        if !paths.is_empty() {
            return Ok(true);
        }
        return git_head_changed_since(project_root, start_head.as_deref());
    }

    if git_head_changed_since(project_root, start_head.as_deref())? {
        return Ok(true);
    }
    git_worktree_has_status_changes(project_root)
}

fn session_start_head(project_root: &Path, session_id: &str) -> Option<String> {
    csa_session::load_session(project_root, session_id)
        .ok()
        .and_then(|session| session.git_head_at_creation)
        .filter(|head| !head.trim().is_empty())
}

fn git_head_changed_since(project_root: &Path, start_head: Option<&str>) -> Result<bool> {
    let Some(start_head) = start_head.map(str::trim).filter(|head| !head.is_empty()) else {
        return Ok(false);
    };

    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .with_context(|| {
            format!(
                "failed to inspect git HEAD for post-exec gate in {}",
                project_root.display()
            )
        })?;

    if !output.status.success() {
        return Ok(true);
    }

    let current_head = String::from_utf8_lossy(&output.stdout);
    Ok(current_head.trim() != start_head)
}

fn current_git_head(project_root: &Path) -> Result<Option<String>> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .with_context(|| {
            format!(
                "failed to inspect git HEAD for post-exec gate in {}",
                project_root.display()
            )
        })?;

    if !output.status.success() {
        return Ok(None);
    }

    let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if head.is_empty() {
        Ok(None)
    } else {
        Ok(Some(head))
    }
}

fn git_worktree_has_status_changes(project_root: &Path) -> Result<bool> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["status", "--porcelain=v1", "--untracked-files=all"])
        .output()
        .with_context(|| {
            format!(
                "failed to inspect git status for post-exec gate in {}",
                project_root.display()
            )
        })?;

    if !output.status.success() {
        return Ok(true);
    }

    Ok(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
}

/// Whether the project worktree has dirty TRACKED changes (unstaged or staged
/// modifications to files git already tracks).
///
/// Untracked files are intentionally excluded: a correct planning-only run
/// (e.g. `--skill mktd`) writes its artifacts to the session output directory
/// outside the repo tree (#1820), so a genuine plan-only run leaves the tracked
/// tree clean. Keying on tracked changes avoids false-positives on generated /
/// session-output scratch that would regress #1819's plan-only gate skip.
///
/// Fails closed: a git command that runs but reports a non-zero / unknown state
/// is treated as dirty so the caller runs the verification gate rather than
/// skipping on an unknown state (rule 009). Only an outright git-spawn failure
/// propagates as an error.
fn project_worktree_has_dirty_tracked_changes(project_root: &Path) -> Result<bool> {
    let quiet_diff_signals_changes = |args: &[&str]| -> Result<bool> {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .with_context(|| {
                format!(
                    "failed to inspect git tracked changes for post-exec gate in {}",
                    project_root.display()
                )
            })?;
        // `git diff --quiet` exits 0 when clean, 1 when differences exist, and
        // >1 on error; any non-zero exit is treated as dirty so the caller
        // fails closed toward running the gate rather than skipping unverified.
        Ok(!status.success())
    };

    // Unstaged tracked modifications, then staged (index) tracked modifications.
    Ok(quiet_diff_signals_changes(&["diff", "--quiet"])?
        || quiet_diff_signals_changes(&["diff", "--cached", "--quiet"])?)
}

fn classify_post_exec_gate_worktree(
    project_root: &Path,
    session_id: Option<&str>,
) -> PostExecGateWorktreeState {
    if !crate::run_cmd::is_git_worktree(project_root) {
        return PostExecGateWorktreeState::DirtyOrUnknown;
    }

    let Some(start_head) = session_id.and_then(|id| session_start_head(project_root, id)) else {
        return PostExecGateWorktreeState::DirtyOrUnknown;
    };

    let Ok(Some(current_head)) = current_git_head(project_root) else {
        return PostExecGateWorktreeState::DirtyOrUnknown;
    };

    if current_head.trim() == start_head.trim() {
        return PostExecGateWorktreeState::DirtyOrUnknown;
    }

    match git_worktree_has_status_changes(project_root) {
        Ok(false) => PostExecGateWorktreeState::CommittedClean,
        Ok(true) | Err(_) => PostExecGateWorktreeState::DirtyOrUnknown,
    }
}

fn strip_inherited_csa_env(cmd: &mut Command) {
    for var in csa_executor::CHILD_PROCESS_STRIPPED_ENV_VARS {
        cmd.env_remove(var);
    }
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("CSA_") {
            cmd.env_remove(key);
        }
    }
}

pub(super) fn execute_post_exec_gate_command(
    command: &str,
    project_root: &Path,
    timeout_seconds: u64,
    extra_env: Option<HashMap<String, String>>,
) -> PostExecGateFuture {
    let command = command.to_string();
    let project_root = project_root.to_path_buf();

    Box::pin(async move {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(&command)
            .current_dir(&project_root)
            // Capture stdout/stderr so a failure can be surfaced structurally
            // (#1726); the tee tasks below re-emit every chunk to the parent's
            // stdout/stderr, so the raw transcript (`full.md`) is unchanged.
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(extra_env) = extra_env {
            cmd.envs(extra_env);
        }
        strip_inherited_csa_env(&mut cmd);

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

        // Tee both streams: re-emit to the parent while accumulating a combined
        // copy for the structured failure report.
        let captured = Arc::new(Mutex::new(Vec::<u8>::new()));
        let stdout_pump = child
            .stdout
            .take()
            .map(|reader| tee_gate_stream(reader, tokio::io::stdout(), Arc::clone(&captured)));
        let stderr_pump = child
            .stderr
            .take()
            .map(|reader| tee_gate_stream(reader, tokio::io::stderr(), Arc::clone(&captured)));

        let exit =
            match tokio::time::timeout(Duration::from_secs(timeout_seconds), child.wait()).await {
                Ok(wait_result) => {
                    let status = wait_result.with_context(|| {
                        format!(
                            "failed while waiting for post-exec gate command `{command}` in {}",
                            project_root.display()
                        )
                    })?;
                    PostExecGateCommandExit::Exited(status.code())
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
                    PostExecGateCommandExit::TimedOut
                }
            };

        // Drain the tee tasks to EOF before reading the buffer. Killing the
        // process group (or normal exit) closes the child's pipe write ends, so
        // both reads reach EOF and the joins complete.
        if let Some(pump) = stdout_pump {
            let _ = pump.await;
        }
        if let Some(pump) = stderr_pump {
            let _ = pump.await;
        }

        let captured_output = captured
            .lock()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();

        Ok(PostExecGateCommandOutcome {
            exit,
            captured_output,
        })
    })
}

pub(super) async fn maybe_run_post_exec_gate_with_runner<F>(
    project_root: &Path,
    prompt_text: &str,
    session_id: Option<&str>,
    config: Option<&ProjectConfig>,
    changed_paths: Option<&[String]>,
    extra_env: Option<HashMap<String, String>>,
    runner: F,
) -> Result<PostExecGateOutcome>
where
    F: FnOnce(&str, &Path, u64, Option<HashMap<String, String>>) -> PostExecGateFuture,
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
        session_id,
        changed_paths,
    )? {
        return Ok(PostExecGateOutcome::Skipped);
    }

    let branch = super::run_context::current_branch_name(project_root);
    let outcome = runner(
        &gate_config.command,
        project_root,
        gate_config.timeout_seconds,
        extra_env,
    )
    .await?;
    let captured_output = outcome.captured_output;
    match outcome.exit {
        PostExecGateCommandExit::Exited(Some(0)) => Ok(PostExecGateOutcome::Passed),
        PostExecGateCommandExit::Exited(code) => {
            Ok(PostExecGateOutcome::Failed(PostExecGateFailure {
                kind: PostExecGateFailureKind::Exited(code),
                diagnostic: format!(
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
                gate_command: gate_config.command.clone(),
                captured_output,
            }))
        }
        PostExecGateCommandExit::TimedOut => Ok(PostExecGateOutcome::Failed(PostExecGateFailure {
            kind: PostExecGateFailureKind::TimedOut,
            diagnostic: format!(
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
            gate_command: gate_config.command.clone(),
            captured_output,
        })),
    }
}

pub(super) async fn apply_post_exec_gate_after_success_with_runner<F>(
    project_root: &Path,
    prompt_text: &str,
    session_id: Option<&str>,
    config: Option<&ProjectConfig>,
    options: PostExecGateApplyOptions<'_>,
    runner: F,
) -> Result<()>
where
    F: FnOnce(&str, &Path, u64, Option<HashMap<String, String>>) -> PostExecGateFuture,
{
    if options.no_post_exec_gate {
        if let Some(session_id) = session_id {
            crate::run_cmd_post::record_post_exec_gate_skipped_by_flag(project_root, session_id);
        }
        return Ok(());
    }
    if options.planning_only {
        // A planning-mode run (e.g. `--skill mktd`) writes its artifacts to the
        // session output directory outside the repo tree (#1820), so a genuine
        // plan-only run leaves the TRACKED worktree clean. The gate skip is
        // therefore conditioned on EFFECT, not just the skill name.
        match project_worktree_has_dirty_tracked_changes(project_root) {
            // Clean tracked tree: a real plan-only run. Skip the code commit
            // gate, preserving #1819's intent that such a session is not failed
            // by `just pre-commit` / check-chinese.
            Ok(false) => return Ok(()),
            // Dirty tracked changes: the run unexpectedly edited tracked source.
            // Record the anomaly and fall through to verify the edits via the
            // configured gate instead of skipping them unverified.
            Ok(true) => {
                if let Some(session_id) = session_id {
                    crate::run_cmd_post::record_post_exec_gate_planning_dirty_override(
                        project_root,
                        session_id,
                    );
                }
            }
            // Fail closed (rule 009): the worktree state is unknown, so never
            // skip. Surface it as a gate failure so orchestrators reading
            // result.toml never observe a false success, then propagate.
            Err(err) => {
                if let Some(session_id) = session_id {
                    crate::run_cmd_post::overwrite_result_as_post_exec_gate_failure(
                        project_root,
                        session_id,
                        &format!("could not inspect worktree for planning-mode gate: {err}"),
                        false,
                    );
                }
                return Err(err);
            }
        }
    }

    let gate_outcome = match maybe_run_post_exec_gate_with_runner(
        project_root,
        prompt_text,
        session_id,
        config,
        options.changed_paths,
        options.extra_env,
        runner,
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(err) => {
            if let Some(session_id) = session_id {
                crate::run_cmd_post::overwrite_result_as_post_exec_gate_failure(
                    project_root,
                    session_id,
                    &format!("could not run the post-exec gate: {err}"),
                    false,
                );
            }
            return Err(err);
        }
    };

    match gate_outcome {
        PostExecGateOutcome::Passed | PostExecGateOutcome::Skipped => Ok(()),
        PostExecGateOutcome::Failed(failure) if failure.is_timeout() => {
            if classify_post_exec_gate_worktree(project_root, session_id)
                == PostExecGateWorktreeState::CommittedClean
            {
                if let Some(session_id) = session_id {
                    crate::run_cmd_post::record_post_exec_gate_timeout_advisory(
                        project_root,
                        session_id,
                    );
                }
                Ok(())
            } else {
                if let Some(session_id) = session_id {
                    crate::run_cmd_post::overwrite_result_as_post_exec_gate_failure(
                        project_root,
                        session_id,
                        "timeout left dirty/uncommitted work unverified",
                        true,
                    );
                }
                Err(failure.into_error())
            }
        }
        PostExecGateOutcome::Failed(failure) => {
            // Primary surfacing path (#1726): a gate that ran and exited nonzero.
            // Persist the full (redacted) output to `output/gate-failure.log`, a
            // bounded `[post_exec_gate]` table to result.toml, and a banner that
            // makes the employee's pre-gate self-report read as superseded — so
            // an orchestrator that cannot read the raw transcript can still
            // diagnose the failure. (Timeout and infra-error paths above keep the
            // existing simple overwrite; their verdicts are already
            // non-contradictory and carry no structured gate output to surface.)
            if let Some(session_id) = session_id {
                crate::run_cmd_post_gate_report::persist_gate_failure_detail(
                    crate::run_cmd_post_gate_report::GateFailureDetail {
                        project_root,
                        session_id,
                        gate_command: &failure.gate_command,
                        exit_code: failure.report_exit_code(),
                        captured_output: &failure.captured_output,
                    },
                );
            }
            Err(failure.into_error())
        }
    }
}

#[cfg(test)]
#[path = "run_cmd_execute_post_exec_tests.rs"]
mod post_exec_tests;
