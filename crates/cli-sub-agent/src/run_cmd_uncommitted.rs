//! Writer-session dirty-worktree signal for `csa run`.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use csa_config::{RunLargeDiffWarningConfig, RunLargeDiffWarningMode};
use tracing::warn;

const MAX_UNCOMMITTED_FILES: usize = 20;
const REQUIRE_COMMIT_REASON: &str =
    "require-commit contract failed: no qualifying commit or tracked dirty work remains";
const REQUIRE_COMMIT_RECOVERY_ACTION: &str = "inspect_changed_paths_then_commit_or_revert";
const REQUIRE_COMMIT_BLOCKER_SUMMARY_MAX_CHARS: usize = 240;
const REDACTED_PATH: &str = "[redacted-path]";
const LARGE_DIFF_WARNING_TEXT: &str = "This CSA session left a large changed surface. Do not proceed directly to a single commit/PR unless this was explicitly intended. First inspect the file list, split into atomic logical units if possible, and run review per unit. If intentionally large, record that rationale in the commit/PR.";

#[path = "run_cmd_uncommitted_diff_tokens.rs"]
mod diff_tokens;
#[path = "run_cmd_uncommitted_memory_soft_limit.rs"]
mod memory_soft_limit_recovery;
#[path = "run_cmd_uncommitted_require_commit.rs"]
mod require_commit;

#[cfg(test)]
use diff_tokens::{DIFF_BYTES_PER_TOKEN, estimate_diff_stream_tokens, tracked_diff_byte_limit};
use diff_tokens::{
    default_tracked_diff_token_threshold, estimate_changed_surface_tokens,
    tracked_diff_token_threshold,
};

pub(crate) fn is_writer_session(sa_mode: bool, task_type: Option<&str>) -> bool {
    !sa_mode && matches!(task_type, Some("run"))
}

pub(crate) fn effective_writer_must_commit(
    cli_require_commit: bool,
    config: Option<&csa_config::ProjectConfig>,
) -> bool {
    cli_require_commit || config.is_some_and(|cfg| cfg.run.writer_must_commit)
}

#[cfg(test)]
pub(crate) fn summarize_uncommitted_changes(
    porcelain: &str,
    numstat: &str,
) -> Option<csa_session::UncommittedChanges> {
    summarize_uncommitted_changes_with_stats(porcelain, numstat, 0, 0, None)
}

fn summarize_uncommitted_changes_with_stats(
    porcelain: &str,
    numstat: &str,
    extra_insertions: u64,
    approx_diff_tokens: usize,
    path_filter: Option<&BTreeSet<String>>,
) -> Option<csa_session::UncommittedChanges> {
    let mut paths = changed_paths_from_porcelain(porcelain);
    if let Some(filter) = path_filter {
        paths.retain(|path| filter.contains(path));
    }
    if paths.is_empty() {
        return None;
    }

    let (insertions, deletions) = parse_numstat_totals(numstat);
    let file_count = paths.len();
    let files = paths
        .into_iter()
        .take(MAX_UNCOMMITTED_FILES)
        .collect::<Vec<_>>();
    let truncated = file_count.saturating_sub(files.len());

    Some(csa_session::UncommittedChanges {
        file_count,
        insertions: insertions.saturating_add(extra_insertions),
        deletions,
        approx_diff_tokens,
        files,
        truncated,
    })
}

pub(crate) fn large_diff_warning_report(
    changes: &csa_session::UncommittedChanges,
    config: &RunLargeDiffWarningConfig,
) -> Option<csa_session::LargeDiffWarningReport> {
    if !config.enabled || config.mode != RunLargeDiffWarningMode::Warn {
        return None;
    }
    let changed_lines = changes.changed_lines();
    let exceeds_files = config.changed_files > 0 && changes.file_count > config.changed_files;
    let exceeds_lines = config.changed_lines > 0 && changed_lines > config.changed_lines;
    let exceeds_tokens =
        config.approx_diff_tokens > 0 && changes.approx_diff_tokens > config.approx_diff_tokens;
    if !(exceeds_files || exceeds_lines || exceeds_tokens) {
        return None;
    }
    Some(csa_session::LargeDiffWarningReport {
        changed_files: changes.file_count,
        changed_lines,
        approx_diff_tokens: changes.approx_diff_tokens,
    })
}

pub(crate) fn format_large_diff_warning_block(
    warning: &csa_session::LargeDiffWarningReport,
) -> String {
    format!(
        "<!-- CSA:LARGE_DIFF_WARNING changed_files={} changed_lines={} approx_diff_tokens={} -->\n{}\n<!-- CSA:LARGE_DIFF_WARNING:END -->",
        warning.changed_files,
        warning.changed_lines,
        warning.approx_diff_tokens,
        LARGE_DIFF_WARNING_TEXT
    )
}

pub(crate) fn record_run_dirty(
    project_root: &Path,
    session_id: Option<&str>,
    result: &mut csa_process::ExecutionResult,
    changed_paths: Option<&[String]>,
    commit_created: Option<bool>,
    cli_require_commit: bool,
    config: Option<&csa_config::ProjectConfig>,
) -> Option<csa_session::LargeDiffWarningReport> {
    let sa_mode = std::env::var(crate::pipeline::prompt_guard::PROMPT_GUARD_CALLER_INJECTION_ENV)
        .ok()
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1"))
        .unwrap_or(false);
    let large_diff_config = config
        .map(|cfg| cfg.run.large_diff_warning.clone())
        .unwrap_or_default();
    record_writer_uncommitted_changes_with_config(
        project_root,
        session_id,
        result,
        WriterUncommittedRecord {
            sa_mode,
            require_commit: effective_writer_must_commit(cli_require_commit, config),
            changed_paths,
            commit_created,
            large_diff_config: &large_diff_config,
        },
    )
}

struct WriterUncommittedRecord<'a> {
    sa_mode: bool,
    require_commit: bool,
    changed_paths: Option<&'a [String]>,
    commit_created: Option<bool>,
    large_diff_config: &'a RunLargeDiffWarningConfig,
}

fn record_writer_uncommitted_changes_with_config(
    project_root: &Path,
    session_id: Option<&str>,
    result: &mut csa_process::ExecutionResult,
    record: WriterUncommittedRecord<'_>,
) -> Option<csa_session::LargeDiffWarningReport> {
    if !is_writer_session(record.sa_mode, Some("run")) && !record.require_commit {
        return None;
    }
    let token_threshold = tracked_diff_token_threshold(record.large_diff_config);
    let full_changes =
        collect_uncommitted_changes_with_token_threshold(project_root, token_threshold);
    let dirty_tracked_probe = record
        .require_commit
        .then(|| require_commit::inspect_dirty_tracked_changes(project_root));
    let dirty_tracked_changes = dirty_tracked_probe
        .as_ref()
        .and_then(|probe| probe.changes());
    let clean_tree_verification_failure = dirty_tracked_probe
        .as_ref()
        .and_then(|probe| probe.blocker_summary());
    let changes = record
        .changed_paths
        .map(|paths| {
            collect_uncommitted_changes_for_changed_paths_with_token_threshold(
                project_root,
                paths,
                token_threshold,
            )
        })
        .unwrap_or_else(|| full_changes.clone());
    let warning = changes
        .as_ref()
        .and_then(|changes| large_diff_warning_report(changes, record.large_diff_config));
    let commit_created = record.commit_created.unwrap_or(false);
    let require_commit_contract_failure = record.require_commit
        && (!commit_created
            || dirty_tracked_probe
                .as_ref()
                .is_some_and(|probe| !probe.is_clean()));
    let contract_changes = dirty_tracked_changes;

    let maybe_signal_exit = matches!(result.exit_code, 137 | 143);
    if changes.is_none() && !require_commit_contract_failure && !maybe_signal_exit {
        return warning;
    }

    let Some(session_id) = session_id else {
        if require_commit_contract_failure {
            mark_require_commit_contract_failure(result);
        }
        return warning;
    };

    match csa_session::load_result(project_root, session_id) {
        Ok(Some(mut session_result)) => {
            let memory_soft_limit_recovery = memory_soft_limit_recovery::build_recovery_diagnostic(
                project_root,
                &session_result,
                changes.as_ref(),
                record.changed_paths,
                record.commit_created,
                record.require_commit,
            );
            let recovery = require_commit_contract_failure.then(|| {
                build_require_commit_recovery_diagnostic_for_state(
                    &session_result,
                    contract_changes,
                    commit_created,
                    result.csa_gate_failure.as_deref(),
                    clean_tree_verification_failure,
                )
            });
            let mut should_save = false;
            let result_changes = if require_commit_contract_failure {
                dirty_tracked_changes.cloned().or_else(|| changes.clone())
            } else {
                changes.clone()
            };
            if let Some(changes) = result_changes {
                apply_uncommitted_changes_to_result(
                    &mut session_result,
                    changes,
                    warning.clone(),
                    require_commit_contract_failure,
                    recovery,
                );
                should_save = true;
            } else if let Some(recovery) = recovery {
                apply_require_commit_contract_failure_to_result(&mut session_result, recovery);
                should_save = true;
            }
            if let Some(recovery) = memory_soft_limit_recovery {
                session_result.memory_soft_limit_recovery = Some(recovery);
                should_save = true;
            }
            if !should_save {
                return warning;
            }
            if let Err(err) = csa_session::save_result(project_root, session_id, &session_result) {
                warn!(
                    session = %session_id,
                    error = %err,
                    "Failed to persist writer uncommitted-changes signal"
                );
            }
        }
        Ok(None) => {
            warn!(
                session = %session_id,
                "No result.toml to annotate with writer uncommitted-changes signal"
            );
        }
        Err(err) => {
            warn!(
                session = %session_id,
                error = %err,
                "Failed to load result.toml for writer uncommitted-changes signal"
            );
        }
    }

    if require_commit_contract_failure {
        mark_require_commit_contract_failure(result);
    }
    warning
}

pub(crate) fn apply_uncommitted_changes_to_result(
    result: &mut csa_session::SessionResult,
    changes: csa_session::UncommittedChanges,
    large_diff_warning: Option<csa_session::LargeDiffWarningReport>,
    require_commit_contract_failure: bool,
    recovery: Option<csa_session::RequireCommitRecoveryDiagnostic>,
) {
    result.uncommitted_changes = Some(changes);
    result.large_diff_warning = large_diff_warning;
    result.require_commit_recovery = recovery;
    if require_commit_contract_failure {
        let recovery = result.require_commit_recovery.take().unwrap_or_else(|| {
            build_require_commit_recovery_diagnostic_for_state(
                result,
                result.uncommitted_changes.as_ref(),
                false,
                None,
                None,
            )
        });
        apply_require_commit_contract_failure_to_result(result, recovery);
    }
}

fn apply_require_commit_contract_failure_to_result(
    result: &mut csa_session::SessionResult,
    recovery: csa_session::RequireCommitRecoveryDiagnostic,
) {
    remove_incidental_downgrade_warnings(&mut result.warnings);
    result.exit_code = 1;
    result.status = csa_session::SessionResult::status_from_exit_code(1);
    result.summary = REQUIRE_COMMIT_REASON.to_string();
    result.require_commit_recovery = Some(recovery);
}

#[cfg(test)]
fn build_require_commit_recovery_diagnostic(
    result: &csa_session::SessionResult,
    changes: &csa_session::UncommittedChanges,
) -> csa_session::RequireCommitRecoveryDiagnostic {
    build_require_commit_recovery_diagnostic_for_state(result, Some(changes), false, None, None)
}

fn build_require_commit_recovery_diagnostic_for_state(
    result: &csa_session::SessionResult,
    changes: Option<&csa_session::UncommittedChanges>,
    commit_created: bool,
    gate_failure: Option<&str>,
    clean_tree_verification_failure: Option<&str>,
) -> csa_session::RequireCommitRecoveryDiagnostic {
    let termination_exit_code = result.raw_process_exit_code.unwrap_or(result.exit_code);
    let termination_status = result
        .raw_process_exit_code
        .map(raw_termination_status_from_exit_code)
        .unwrap_or_else(|| result.status.clone());
    csa_session::RequireCommitRecoveryDiagnostic {
        require_commit: true,
        commit_created,
        dirty_worktree: changes.is_some(),
        changed_paths: changes
            .map(|changes| {
                changes
                    .files
                    .iter()
                    .map(|path| sanitize_diagnostic_path(path))
                    .collect()
            })
            .unwrap_or_default(),
        changed_paths_truncated: changes.map(|changes| changes.truncated).unwrap_or_default(),
        termination_status,
        exit_code: termination_exit_code,
        termination_signal: result
            .kill_diagnostics
            .as_ref()
            .and_then(|diagnostics| diagnostics.signal)
            .or_else(|| infer_signal_from_exit_code(termination_exit_code)),
        kill_hint: result.kill_hint.clone(),
        blocker_summary: require_commit::build_blocker_summary(
            result,
            gate_failure,
            clean_tree_verification_failure,
        ),
        suggested_recovery_action: REQUIRE_COMMIT_RECOVERY_ACTION.to_string(),
    }
}

fn mark_require_commit_contract_failure(result: &mut csa_process::ExecutionResult) {
    result.mark_gate_failure("writer-uncommitted");
    result.summary = REQUIRE_COMMIT_REASON.to_string();
    if !result.stderr_output.is_empty() && !result.stderr_output.ends_with('\n') {
        result.stderr_output.push('\n');
    }
    result.stderr_output.push_str(REQUIRE_COMMIT_REASON);
    result.stderr_output.push('\n');
}

fn raw_termination_status_from_exit_code(exit_code: i32) -> String {
    match exit_code {
        0 => "success".to_string(),
        124 => "timeout".to_string(),
        137 | 143 => "signal".to_string(),
        _ => "failure".to_string(),
    }
}

fn remove_incidental_downgrade_warnings(warnings: &mut Vec<String>) {
    warnings.retain(|warning| !is_incidental_downgrade_warning(warning));
}

fn is_incidental_downgrade_warning(warning: &str) -> bool {
    warning.contains("incidental nonzero exit") && warning.contains("treated as success")
}

fn infer_signal_from_exit_code(exit_code: i32) -> Option<i32> {
    (129..=255).contains(&exit_code).then_some(exit_code - 128)
}

fn sanitize_diagnostic_path(path: &str) -> String {
    let path = path.strip_prefix("./").unwrap_or(path);
    if path.is_empty() || path.starts_with('/') || has_unsafe_path_component(path) {
        return REDACTED_PATH.to_string();
    }

    path.chars()
        .map(|ch| if ch.is_control() { '�' } else { ch })
        .collect()
}

fn has_unsafe_path_component(path: &str) -> bool {
    path.split('/').any(|part| matches!(part, ".." | ".git"))
}

pub(crate) fn format_uncommitted_warning(changes: &csa_session::UncommittedChanges) -> String {
    format!(
        "⚠ writer session ended with {} uncommitted files (+{}/-{}) — work NOT committed",
        changes.file_count, changes.insertions, changes.deletions
    )
}

/// Total working-tree change size, in lines, used by the review-aware writer
/// guard (#1842) to size-gate prompt injection for resume sessions: tracked diff
/// lines (insertions + deletions vs `HEAD`) PLUS the line count of untracked,
/// non-ignored files.
///
/// Untracked files never appear in `git diff HEAD`, so a substantial change made
/// entirely of new (never-staged) files would otherwise measure as zero changed
/// lines and be mis-classified as trivial — handing the writer the *brief* guard
/// exactly when the full per-dimension checklist matters most. Counting untracked
/// non-ignored lines closes that gap. `.gitignore` is honored (build artifacts do
/// not inflate the count), and git state is never mutated to measure it (no
/// `git add`, no intent-to-add).
/// If untracked sizing proves the line total is only a lower bound (large,
/// binary, capped, or truncated files), this returns `usize::MAX` so the caller's
/// size gate fails toward the full guard instead of treating an unknown exact
/// line count as trivial.
///
/// Returns `0` when `project_root` is not a git worktree or the tree is clean.
/// Any git/IO failure is non-fatal (fail-open), matching the rest of the guard.
pub(crate) fn working_tree_changed_lines(project_root: &Path) -> usize {
    collect_uncommitted_changes_with_filter_and_untracked_size(project_root, None, None)
        .map(|(changes, untracked)| {
            let changed_lines = usize::try_from(changes.changed_lines()).unwrap_or(usize::MAX);
            if untracked.lower_bound {
                usize::MAX
            } else {
                changed_lines
            }
        })
        .unwrap_or(0)
}

#[cfg(test)]
fn collect_uncommitted_changes(project_root: &Path) -> Option<csa_session::UncommittedChanges> {
    collect_uncommitted_changes_with_token_threshold(
        project_root,
        default_tracked_diff_token_threshold(),
    )
}

fn collect_uncommitted_changes_with_token_threshold(
    project_root: &Path,
    token_threshold: usize,
) -> Option<csa_session::UncommittedChanges> {
    collect_uncommitted_changes_with_filter(project_root, None, Some(token_threshold))
}

pub(crate) fn collect_uncommitted_changes_for_changed_paths(
    project_root: &Path,
    changed_paths: &[String],
) -> Option<csa_session::UncommittedChanges> {
    let filter = changed_paths
        .iter()
        .filter(|path| !path.is_empty())
        .cloned()
        .collect::<BTreeSet<_>>();
    if filter.is_empty() {
        return None;
    }
    collect_uncommitted_changes_with_filter(
        project_root,
        Some(&filter),
        Some(default_tracked_diff_token_threshold()),
    )
}

fn collect_uncommitted_changes_for_changed_paths_with_token_threshold(
    project_root: &Path,
    changed_paths: &[String],
    token_threshold: usize,
) -> Option<csa_session::UncommittedChanges> {
    let filter = changed_paths
        .iter()
        .filter(|path| !path.is_empty())
        .cloned()
        .collect::<BTreeSet<_>>();
    if filter.is_empty() {
        return None;
    }
    collect_uncommitted_changes_with_filter(project_root, Some(&filter), Some(token_threshold))
}

fn collect_uncommitted_changes_with_filter(
    project_root: &Path,
    path_filter: Option<&BTreeSet<String>>,
    tracked_token_threshold: Option<usize>,
) -> Option<csa_session::UncommittedChanges> {
    collect_uncommitted_changes_with_filter_and_untracked_size(
        project_root,
        path_filter,
        tracked_token_threshold,
    )
    .map(|(changes, _)| changes)
}

fn collect_uncommitted_changes_with_filter_and_untracked_size(
    project_root: &Path,
    path_filter: Option<&BTreeSet<String>>,
    tracked_token_threshold: Option<usize>,
) -> Option<(
    csa_session::UncommittedChanges,
    crate::untracked_size::UntrackedDiffSize,
)> {
    if !super::git::is_git_worktree(project_root) {
        return None;
    }

    let porcelain = run_git_capture(
        project_root,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--no-renames",
            "-z",
        ],
    )?;
    if porcelain.is_empty() {
        return None;
    }
    let numstat = run_git_diff_capture(project_root, &["diff", "--numstat", "HEAD"], path_filter)
        .unwrap_or_default();
    let untracked = match path_filter {
        Some(filter) => crate::untracked_size::untracked_diff_size_for_paths(project_root, filter),
        None => crate::untracked_size::untracked_diff_size(project_root),
    };
    let untracked_lines = u64::try_from(untracked.lines).unwrap_or(u64::MAX);
    let approx_diff_tokens = estimate_changed_surface_tokens(
        project_root,
        path_filter,
        &untracked,
        tracked_token_threshold,
    );
    let changes = summarize_uncommitted_changes_with_stats(
        &porcelain,
        &numstat,
        untracked_lines,
        approx_diff_tokens,
        path_filter,
    )?;
    Some((changes, untracked))
}

fn run_git_capture(project_root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

fn run_git_diff_capture(
    project_root: &Path,
    args: &[&str],
    path_filter: Option<&BTreeSet<String>>,
) -> Option<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(project_root).args(args).arg("--");
    if let Some(filter) = path_filter {
        command.env("GIT_LITERAL_PATHSPECS", "1");
        command.args(filter);
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

fn changed_paths_from_porcelain(porcelain: &str) -> Vec<String> {
    porcelain_entries(porcelain)
        .into_iter()
        .filter_map(parse_porcelain_path)
        .collect()
}

fn porcelain_entries(porcelain: &str) -> Vec<&str> {
    if porcelain.contains('\0') {
        porcelain
            .split('\0')
            .filter(|entry| !entry.is_empty())
            .collect()
    } else {
        porcelain
            .lines()
            .filter(|entry| !entry.is_empty())
            .collect()
    }
}

fn parse_porcelain_path(entry: &str) -> Option<String> {
    let mut chars = entry.chars();
    chars.next()?;
    chars.next()?;
    if chars.next()? != ' ' {
        return None;
    }
    let path = entry.get(3..)?;
    (!path.is_empty()).then(|| path.to_string())
}

fn parse_numstat_totals(numstat: &str) -> (u64, u64) {
    let mut insertions = 0u64;
    let mut deletions = 0u64;

    for line in numstat.lines() {
        let mut columns = line.split('\t');
        let added = columns.next().and_then(|raw| raw.parse::<u64>().ok());
        let removed = columns.next().and_then(|raw| raw.parse::<u64>().ok());
        if let Some(value) = added {
            insertions = insertions.saturating_add(value);
        }
        if let Some(value) = removed {
            deletions = deletions.saturating_add(value);
        }
    }

    (insertions, deletions)
}

#[cfg(test)]
#[path = "run_cmd_uncommitted_incidental_tests.rs"]
mod incidental_tests;

#[cfg(test)]
#[path = "run_cmd_uncommitted_memory_soft_limit_tests.rs"]
mod memory_soft_limit_tests;

#[cfg(test)]
#[path = "run_cmd_uncommitted_require_commit_tests.rs"]
mod require_commit_tests;

#[cfg(test)]
#[path = "run_cmd_uncommitted_tests.rs"]
mod tests;
