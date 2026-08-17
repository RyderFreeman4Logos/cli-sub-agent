use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

const SANDBOX_COMMIT_FAILURE_MARKER_MAX_BYTES: usize = 1024;
const REQUIRE_COMMIT_GIT_PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const SANDBOX_HOOK_PROBE_REASON: &str = "require-commit blocked: sandbox hook failure state could not be verified; staged tree preserved for host recovery";

#[derive(Debug, Clone, Copy)]
pub(super) enum SandboxHookProbeState<'a> {
    Clear,
    Blocked,
    Uncertain(&'a str),
}

impl<'a> SandboxHookProbeState<'a> {
    pub(super) fn from_result(probe: Option<&'a Result<bool, String>>) -> Self {
        match probe {
            Some(Ok(true)) => Self::Blocked,
            Some(Err(error)) => Self::Uncertain(error),
            Some(Ok(false)) | None => Self::Clear,
        }
    }

    fn requires_host_recovery(self) -> bool {
        !matches!(self, Self::Clear)
    }
}

pub(super) fn contract_failure_reason(state: SandboxHookProbeState<'_>) -> &'static str {
    match state {
        SandboxHookProbeState::Blocked => super::REQUIRE_COMMIT_SANDBOX_HOOK_REASON,
        SandboxHookProbeState::Uncertain(_) => SANDBOX_HOOK_PROBE_REASON,
        SandboxHookProbeState::Clear => super::REQUIRE_COMMIT_REASON,
    }
}

pub(super) fn persisted_contract_failure_reason(
    recovery: &csa_session::RequireCommitRecoveryDiagnostic,
) -> &'static str {
    if recovery
        .blocker_summary
        .as_deref()
        .is_some_and(|summary| summary.contains("sandbox_hook_probe="))
    {
        SANDBOX_HOOK_PROBE_REASON
    } else if recovery.suggested_recovery_action
        == super::REQUIRE_COMMIT_SANDBOX_HOOK_RECOVERY_ACTION
    {
        super::REQUIRE_COMMIT_SANDBOX_HOOK_REASON
    } else {
        super::REQUIRE_COMMIT_REASON
    }
}

#[derive(Debug, Clone)]
pub(super) enum DirtyTrackedWorktree {
    Clean,
    Dirty(csa_session::UncommittedChanges),
    Unknown { blocker_summary: String },
}

impl DirtyTrackedWorktree {
    pub(super) fn is_clean(&self) -> bool {
        matches!(self, Self::Clean)
    }

    pub(super) fn changes(&self) -> Option<&csa_session::UncommittedChanges> {
        match self {
            Self::Dirty(changes) => Some(changes),
            Self::Clean | Self::Unknown { .. } => None,
        }
    }

    pub(super) fn blocker_summary(&self) -> Option<&str> {
        match self {
            Self::Unknown { blocker_summary } => Some(blocker_summary.as_str()),
            Self::Clean | Self::Dirty(_) => None,
        }
    }
}

pub(super) fn build_blocker_summary(
    result: &csa_session::SessionResult,
    gate_failure: Option<&str>,
    clean_tree_verification_failure: Option<&str>,
    sandbox_hook_state: SandboxHookProbeState<'_>,
) -> Option<String> {
    let mut parts = Vec::new();
    match sandbox_hook_state {
        SandboxHookProbeState::Blocked => parts.push(
            "sandbox_hook=mandatory hook-enabled commit failed for unchanged staged tree"
                .to_string(),
        ),
        SandboxHookProbeState::Uncertain(error) => {
            parts.push(format!("sandbox_hook_probe={error}"));
        }
        SandboxHookProbeState::Clear => {}
    }
    if let Some(gate_failure) = gate_failure
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        parts.push(format!("gate={gate_failure}"));
    }
    if let Some(clean_tree_verification_failure) = clean_tree_verification_failure
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        parts.push(format!(
            "clean_tree_verification={clean_tree_verification_failure}"
        ));
    }
    let trimmed_summary = result.summary.trim();
    let summary = trimmed_summary
        .strip_prefix("Summary: ")
        .unwrap_or(trimmed_summary)
        .trim();
    if !summary.is_empty() && summary != super::REQUIRE_COMMIT_REASON {
        parts.push(format!("summary={summary}"));
    }
    if parts.is_empty() {
        return None;
    }
    Some(bound_redacted_one_line(
        &parts.join("; "),
        super::REQUIRE_COMMIT_BLOCKER_SUMMARY_MAX_CHARS,
    ))
}

fn bound_redacted_one_line(value: &str, max_chars: usize) -> String {
    let redacted = csa_session::redact_text_content(value);
    let one_line = redacted.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max_chars {
        return one_line;
    }
    let keep_chars = max_chars.saturating_sub(3);
    let mut truncated = one_line.chars().take(keep_chars).collect::<String>();
    truncated = truncated.trim_end().to_string();
    truncated.push_str("...");
    truncated
}

pub(super) fn build_recovery_diagnostic_for_state(
    result: &csa_session::SessionResult,
    changes: Option<&csa_session::UncommittedChanges>,
    commit_created: bool,
    gate_failure: Option<&str>,
    clean_tree_verification_failure: Option<&str>,
    sa_mode: Option<bool>,
    sandbox_hook_state: SandboxHookProbeState<'_>,
) -> csa_session::RequireCommitRecoveryDiagnostic {
    let termination_exit_code = result.raw_process_exit_code.unwrap_or(result.exit_code);
    let termination_status = result
        .raw_process_exit_code
        .map(super::raw_termination_status_from_exit_code)
        .unwrap_or_else(|| result.status.clone());
    csa_session::RequireCommitRecoveryDiagnostic {
        require_commit: true,
        sa_mode,
        commit_created,
        dirty_worktree: changes.is_some(),
        changed_paths: changes
            .map(|changes| {
                changes
                    .files
                    .iter()
                    .map(|path| super::sanitize_diagnostic_path(path))
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
            .or_else(|| super::infer_signal_from_exit_code(termination_exit_code)),
        kill_hint: result.kill_hint.clone(),
        blocker_summary: build_blocker_summary(
            result,
            gate_failure,
            clean_tree_verification_failure,
            sandbox_hook_state,
        ),
        suggested_recovery_action: if sandbox_hook_state.requires_host_recovery() {
            super::REQUIRE_COMMIT_SANDBOX_HOOK_RECOVERY_ACTION
        } else {
            super::REQUIRE_COMMIT_RECOVERY_ACTION
        }
        .to_string(),
    }
}

pub(super) fn sandbox_commit_failure_matches(
    project_root: &Path,
    session_id: &str,
) -> Result<bool, String> {
    let session_dir = csa_session::get_session_dir(project_root, session_id)
        .map_err(|_| "sandbox-hook-marker-session-dir-unavailable".to_string())?;
    let Some(marker) = read_sandbox_commit_failure_marker(
        &session_dir.join(csa_hooks::git_guard::SANDBOX_COMMIT_FAILURE_MARKER_FILE),
    )?
    else {
        return Ok(false);
    };
    let fields = marker.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 6 {
        return Err("sandbox-hook-marker-invalid-record".to_string());
    }
    let head_output = run_git_output(project_root, &["rev-parse", "--verify", "HEAD"])
        .ok_or_else(|| "sandbox-hook-marker-head-probe-spawn-or-timeout".to_string())?;
    let current_head = if head_output.status.success() {
        String::from_utf8_lossy(&head_output.stdout)
            .trim()
            .to_string()
    } else if head_output.status.code() == Some(128) {
        "unborn".to_string()
    } else {
        return Err(format!(
            "sandbox-hook-marker-head-probe-failed exit_code={}",
            head_output
                .status
                .code()
                .map_or_else(|| "unknown".to_string(), |code| code.to_string())
        ));
    };
    if fields[0] != current_head {
        return Ok(false);
    }
    let current_index_tree = run_git_status_porcelain(project_root, &["write-tree"])
        .map_err(|error| format!("sandbox-hook-marker-index-probe={error}"))?;
    Ok(fields[1] == current_index_tree.trim())
}

#[cfg(unix)]
fn read_sandbox_commit_failure_marker(path: &Path) -> Result<Option<String>, String> {
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("sandbox-hook-marker-open-failed".to_string()),
    };
    let metadata = file
        .metadata()
        .map_err(|_| "sandbox-hook-marker-metadata-failed".to_string())?;
    if !metadata.is_file()
        || metadata.len()
            > u64::try_from(SANDBOX_COMMIT_FAILURE_MARKER_MAX_BYTES)
                .map_err(|_| "sandbox-hook-marker-size-limit-invalid".to_string())?
    {
        return Err("sandbox-hook-marker-not-bounded-regular-file".to_string());
    }

    let mut record = Vec::new();
    let mut reader = file.take(
        u64::try_from(SANDBOX_COMMIT_FAILURE_MARKER_MAX_BYTES + 1)
            .map_err(|_| "sandbox-hook-marker-size-limit-invalid".to_string())?,
    );
    reader
        .read_to_end(&mut record)
        .map_err(|_| "sandbox-hook-marker-read-failed".to_string())?;
    if record.len() > SANDBOX_COMMIT_FAILURE_MARKER_MAX_BYTES {
        return Err("sandbox-hook-marker-too-large".to_string());
    }
    if record.last() != Some(&b'\n') || record[..record.len() - 1].contains(&b'\n') {
        return Err("sandbox-hook-marker-not-single-record".to_string());
    }
    String::from_utf8(record)
        .map(Some)
        .map_err(|_| "sandbox-hook-marker-not-utf8".to_string())
}

#[cfg(not(unix))]
fn read_sandbox_commit_failure_marker(_path: &Path) -> Result<Option<String>, String> {
    Err("sandbox-hook-marker-reader-unavailable".to_string())
}

pub(super) fn inspect_dirty_tracked_changes(project_root: &Path) -> DirtyTrackedWorktree {
    let porcelain = match run_git_status_porcelain(
        project_root,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=no",
            "--no-renames",
            "-z",
        ],
    ) {
        Ok(porcelain) => porcelain,
        Err(blocker_summary) => {
            return DirtyTrackedWorktree::Unknown { blocker_summary };
        }
    };
    if porcelain.is_empty() {
        return DirtyTrackedWorktree::Clean;
    }
    let numstat = super::run_git_diff_capture(project_root, &["diff", "--numstat", "HEAD"], None)
        .unwrap_or_default();
    match super::summarize_uncommitted_changes_with_stats(&porcelain, &numstat, 0, 0, None) {
        Some(changes) => DirtyTrackedWorktree::Dirty(changes),
        None => DirtyTrackedWorktree::Unknown {
            blocker_summary: "git-status-probe-unparseable".to_string(),
        },
    }
}

fn run_git_status_porcelain(project_root: &Path, args: &[&str]) -> Result<String, String> {
    let output = run_git_output(project_root, args)
        .ok_or_else(|| "git-status-probe-spawn-or-timeout".to_string())?;
    if !output.status.success() {
        let exit_code = output
            .status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        return Err(format!("git-status-probe-failed exit_code={exit_code}"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn run_git_output(project_root: &Path, args: &[&str]) -> Option<Output> {
    let mut command = Command::new("git");
    command.arg("-C").arg(project_root).args(args);
    crate::review_cmd::run_command_with_timeout(&mut command, REQUIRE_COMMIT_GIT_PROBE_TIMEOUT)
}
