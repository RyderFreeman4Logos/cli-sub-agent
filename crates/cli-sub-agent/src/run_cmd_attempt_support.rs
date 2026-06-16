use std::path::{Path, PathBuf};
use std::process::Command;

use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{ToolName, ToolSelectionStrategy};

use crate::edit_restriction_guard::{TrackedFileEditGuard, maybe_capture_tracked_file_guard};
use crate::pipeline;

pub(crate) type CommitSkillWorkspaceGuard = Option<CommitSkillWorkspaceSnapshot>;

pub(crate) struct CommitSkillWorkspaceSnapshot {
    project_root: PathBuf,
    pre_head_oid: Option<String>,
    tracked_guard: TrackedFileEditGuard,
}

impl CommitSkillWorkspaceSnapshot {
    fn head_changed(&self) -> anyhow::Result<bool> {
        Ok(git_head_oid(&self.project_root)? != self.pre_head_oid)
    }
}

pub(crate) fn allow_cross_tool_failover(
    strategy: ToolSelectionStrategy,
    _resolved_tier_name: Option<&str>,
    _force_ignore_tier_setting: bool,
    no_failover: bool,
) -> bool {
    if no_failover {
        return false;
    }

    // Without an active tier, explicit `--tool` (from CLI or skill
    // agent_config) is the user's hard selection: never silently fall over to a
    // different tool (#1440). Active tier failover is handled by the scheduler
    // using the resolved tier model specs, not by this generic slot fallback.
    !matches!(strategy, ToolSelectionStrategy::Explicit(_))
}

pub(crate) fn resolve_max_failover_attempts(
    no_failover: bool,
    config: Option<&ProjectConfig>,
) -> usize {
    if no_failover {
        1
    } else {
        config
            .map(|cfg| {
                cfg.tiers
                    .values()
                    .map(|tier| tier.models.len())
                    .sum::<usize>()
                    .max(1)
            })
            .unwrap_or(1)
    }
}

pub(crate) fn resolve_runtime_fallback_enabled(
    strategy: &ToolSelectionStrategy,
    no_failover: bool,
) -> bool {
    matches!(strategy, ToolSelectionStrategy::HeterogeneousPreferred) && !no_failover
}

pub(crate) fn strategy_is_explicit(strategy: &ToolSelectionStrategy) -> bool {
    matches!(strategy, ToolSelectionStrategy::Explicit(_))
}

pub(crate) fn codex_fast_mode_enabled(
    cli_fast_mode: bool,
    config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
) -> bool {
    cli_fast_mode
        || config
            .and_then(|cfg| cfg.tools.get("codex"))
            .and_then(|tool| tool.fast_mode)
            .unwrap_or(false)
        || global_config
            .tools
            .get("codex")
            .and_then(|tool| tool.fast_mode)
            .unwrap_or(false)
}

pub(crate) fn resolve_attempt_initial_response_timeout_seconds(
    config: Option<&ProjectConfig>,
    cli_initial_response_timeout: Option<u64>,
    cli_idle_timeout: Option<u64>,
    no_idle_timeout: bool,
    tool_name: &str,
) -> Option<u64> {
    if no_idle_timeout {
        None
    } else {
        pipeline::resolve_initial_response_timeout_for_tool(
            config,
            cli_initial_response_timeout,
            cli_idle_timeout,
            tool_name,
        )
    }
}

/// Build a prompt addendum for rate-limit failover that tells the new tool
/// how to retrieve the original session's conversation context via xurl.
///
/// Returns `None` when there is no session to reference.
pub(crate) fn build_failover_context_addendum(
    failed_tool: &str,
    session_id: Option<&str>,
) -> Option<String> {
    let sid = session_id?;
    let provider = match failed_tool {
        "gemini-cli" => "gemini",
        "antigravity-cli" => "antigravity",
        "claude-code" => "claude",
        "codex" => "codex",
        "opencode" => "opencode",
        other => other,
    };
    Some(format!(
        "[Rate-limit failover context]\n\
         This task was originally being handled by {failed_tool} (session {sid}) \
         which hit a rate limit / quota exhaustion.\n\
         If you need the full conversation context from the previous session, run:\n\
         ```\n\
         csa xurl threads --keyword {provider}\n\
         ```\n\
         Then use the session/thread ID to read the relevant conversation."
    ))
}

pub(crate) fn merge_retry_changed_paths(
    accumulated_changed_paths: &mut Vec<String>,
    all_attempt_change_snapshots_available: &mut bool,
    attempt_changed_paths: Option<Vec<String>>,
) {
    match attempt_changed_paths {
        Some(paths) => accumulated_changed_paths.extend(paths),
        None => *all_attempt_change_snapshots_available = false,
    }
}

pub(crate) fn merge_run_loop_changed_paths(
    mut accumulated_changed_paths: Vec<String>,
    mut all_attempt_change_snapshots_available: bool,
    final_changed_paths: Option<Vec<String>>,
) -> Option<Vec<String>> {
    match final_changed_paths {
        Some(paths) => accumulated_changed_paths.extend(paths),
        None => all_attempt_change_snapshots_available = false,
    }

    if !all_attempt_change_snapshots_available {
        return None;
    }

    accumulated_changed_paths.sort();
    accumulated_changed_paths.dedup();
    Some(accumulated_changed_paths)
}

pub(crate) fn capture_commit_skill_workspace_guard(
    project_root: &Path,
    skill: Option<&str>,
) -> anyhow::Result<CommitSkillWorkspaceGuard> {
    if skill != Some("commit") {
        return Ok(None);
    }
    let Some(tracked_guard) = maybe_capture_tracked_file_guard(project_root)? else {
        return Ok(None);
    };
    Ok(Some(CommitSkillWorkspaceSnapshot {
        project_root: project_root.to_path_buf(),
        pre_head_oid: git_head_oid(project_root)?,
        tracked_guard,
    }))
}

pub(crate) fn restore_failed_commit_skill_workspace(
    guard: &mut CommitSkillWorkspaceGuard,
    commit_created: Option<bool>,
    result: Option<&mut csa_process::ExecutionResult>,
) -> anyhow::Result<()> {
    if commit_created.unwrap_or(false) {
        return Ok(());
    }

    let Some(snapshot) = guard.take() else {
        return Ok(());
    };
    if snapshot.head_changed()? {
        return Ok(());
    }
    let Some(violation) = snapshot.tracked_guard.enforce_and_restore()? else {
        return Ok(());
    };

    let Some(result) = result else {
        return Ok(());
    };
    if !result.stderr_output.is_empty() && !result.stderr_output.ends_with('\n') {
        result.stderr_output.push('\n');
    }
    result
        .stderr_output
        .push_str("Commit skill workspace guard restored tracked drift after missing commit.\n");
    result.stderr_output.push_str(&violation.detail_message());
    if !result.stderr_output.ends_with('\n') {
        result.stderr_output.push('\n');
    }
    Ok(())
}

fn git_head_oid(project_root: &Path) -> anyhow::Result<Option<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
    ))
}

pub(crate) fn persist_fork_timeout_result_if_missing(
    project_root: &Path,
    is_fork: bool,
    tool: ToolName,
    session_id: Option<&str>,
    execution_start_time: chrono::DateTime<chrono::Utc>,
    timeout_seconds: u64,
) {
    if !is_fork {
        return;
    }
    let Some(session_id) = session_id else {
        return;
    };

    let err = anyhow::anyhow!(
        "wall-clock timeout interrupted forked execution before normal finalization after {timeout_seconds}s"
    );
    crate::pipeline_post_exec::ensure_terminal_result_for_session_on_post_exec_error(
        project_root,
        session_id,
        tool.as_str(),
        execution_start_time,
        &err,
    );
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;

    use super::{
        capture_commit_skill_workspace_guard, merge_run_loop_changed_paths,
        restore_failed_commit_skill_workspace,
    };

    fn run_git(project_root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .output()
            .expect("git command should run");
        assert!(
            output.status.success(),
            "git {} failed\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_status(project_root: &Path) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(["status", "--porcelain"])
            .output()
            .expect("git status should run");
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    fn git_diff_cached(project_root: &Path) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(["diff", "--cached"])
            .output()
            .expect("git diff --cached should run");
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    #[test]
    fn failed_commit_skill_restores_tracked_drift_without_unstaging_existing_diff() {
        let repo = tempfile::tempdir().expect("repo tempdir");
        run_git(repo.path(), &["init", "-q"]);
        run_git(
            repo.path(),
            &["config", "user.email", "csa@example.invalid"],
        );
        run_git(repo.path(), &["config", "user.name", "CSA Test"]);
        std::fs::write(repo.path().join("README.md"), "before\n").expect("seed readme");
        std::fs::write(repo.path().join("weave.lock"), "lock\n").expect("seed lockfile");
        run_git(repo.path(), &["add", "README.md"]);
        run_git(repo.path(), &["add", "-f", "weave.lock"]);
        run_git(repo.path(), &["commit", "-m", "seed"]);

        std::fs::write(repo.path().join("README.md"), "after\n").expect("stage readme");
        run_git(repo.path(), &["add", "README.md"]);
        assert_eq!(git_status(repo.path()), "M  README.md\n");
        let expected_cached_diff = git_diff_cached(repo.path());

        let mut guard = capture_commit_skill_workspace_guard(repo.path(), Some("commit"))
            .expect("guard capture should succeed");
        std::fs::write(repo.path().join("README.md"), "tool rewrite\n")
            .expect("simulate staged rewrite");
        run_git(repo.path(), &["add", "README.md"]);
        std::fs::remove_file(repo.path().join("weave.lock")).expect("simulate stray lock deletion");
        assert_eq!(git_status(repo.path()), "M  README.md\n D weave.lock\n");

        let mut result = csa_process::ExecutionResult::default();
        restore_failed_commit_skill_workspace(&mut guard, Some(false), Some(&mut result))
            .expect("restore should succeed");

        assert_eq!(git_status(repo.path()), "M  README.md\n");
        assert_eq!(git_diff_cached(repo.path()), expected_cached_diff);
        assert_eq!(
            std::fs::read_to_string(repo.path().join("weave.lock")).expect("lock restored"),
            "lock\n"
        );
        assert!(
            result
                .stderr_output
                .contains("Commit skill workspace guard restored tracked drift")
        );
    }

    #[test]
    fn unknown_commit_state_restores_only_when_head_is_unchanged() {
        let repo = tempfile::tempdir().expect("repo tempdir");
        run_git(repo.path(), &["init", "-q"]);
        run_git(
            repo.path(),
            &["config", "user.email", "csa@example.invalid"],
        );
        run_git(repo.path(), &["config", "user.name", "CSA Test"]);
        std::fs::write(repo.path().join("README.md"), "before\n").expect("seed readme");
        std::fs::write(repo.path().join("weave.lock"), "lock\n").expect("seed lockfile");
        run_git(repo.path(), &["add", "README.md"]);
        run_git(repo.path(), &["add", "-f", "weave.lock"]);
        run_git(repo.path(), &["commit", "-m", "seed"]);

        let mut guard = capture_commit_skill_workspace_guard(repo.path(), Some("commit"))
            .expect("guard capture should succeed");
        std::fs::remove_file(repo.path().join("weave.lock")).expect("simulate stray lock deletion");
        restore_failed_commit_skill_workspace(&mut guard, None, None)
            .expect("restore should succeed");
        assert!(git_status(repo.path()).trim().is_empty());

        let mut guard = capture_commit_skill_workspace_guard(repo.path(), Some("commit"))
            .expect("guard capture should succeed");
        std::fs::write(repo.path().join("README.md"), "committed\n").expect("session edit");
        run_git(repo.path(), &["add", "README.md"]);
        run_git(repo.path(), &["commit", "-m", "session commit"]);
        std::fs::remove_file(repo.path().join("weave.lock")).expect("simulate post-commit drift");
        restore_failed_commit_skill_workspace(&mut guard, None, None)
            .expect("restore should skip after HEAD changes");
        assert_eq!(git_status(repo.path()), " D weave.lock\n");
    }

    #[test]
    fn merge_run_loop_changed_paths_preserves_known_empty_delta() {
        let merged = merge_run_loop_changed_paths(Vec::new(), true, Some(Vec::new()));

        assert_eq!(merged, Some(Vec::new()));
    }

    #[test]
    fn merge_run_loop_changed_paths_returns_unknown_when_any_snapshot_is_missing() {
        let merged =
            merge_run_loop_changed_paths(vec!["src/lib.rs".to_string()], false, Some(vec![]));

        assert_eq!(merged, None);
    }

    #[test]
    fn merge_run_loop_changed_paths_deduplicates_known_paths() {
        let merged = merge_run_loop_changed_paths(
            vec!["src/lib.rs".to_string()],
            true,
            Some(vec!["src/lib.rs".to_string(), "src/main.rs".to_string()]),
        );

        assert_eq!(
            merged,
            Some(vec!["src/lib.rs".to_string(), "src/main.rs".to_string()])
        );
    }
}
