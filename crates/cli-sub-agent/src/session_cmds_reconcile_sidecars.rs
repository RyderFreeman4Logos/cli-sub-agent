use anyhow::{Context, Result, anyhow};
use csa_core::vcs::VcsKind;
use csa_session::MetaSessionState;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use tracing::warn;

use crate::plan_cmd::shell_escape_for_command;
use crate::session_result_publish::preserve_existing_permissions_if_present;

use super::reconcile_git::{git_output, git_success, resolve_fallback_base_branch};

const UNPUSHED_COMMITS_SIDECAR_PATH: &str = "output/unpushed_commits.json";

#[rustfmt::skip]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct UnpushedCommitRecord { sha: String, subject: String }

#[rustfmt::skip]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct UnpushedCommitsSidecar { branch: String, remote_ref: Option<String>, commits_ahead: u64, commits: Vec<UnpushedCommitRecord>, recovery_command: String }

#[rustfmt::skip]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ArtifactRollbackGuard { artifact_path: PathBuf, expected_contents: Vec<u8>, rollback_action: ArtifactRollbackAction }

#[rustfmt::skip]
#[derive(Debug, Clone, PartialEq, Eq)]
enum ArtifactRollbackAction { RemoveIfContentsMatch, RestoreOriginal(Vec<u8>) }

fn inspect_unpushed_commits(
    project_root: &Path,
    branch: &str,
) -> Result<Option<UnpushedCommitsSidecar>> {
    let session_branch_ref = format!("refs/heads/{branch}");
    let range = if git_success(
        project_root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/remotes/origin/{branch}"),
        ],
    ) {
        (
            Some(format!("origin/{branch}")),
            format!("origin/{branch}..{session_branch_ref}"),
        )
    } else {
        let Some(base_branch) = resolve_fallback_base_branch(project_root) else {
            return Ok(None);
        };
        (None, format!("{base_branch}..{session_branch_ref}"))
    };
    let (remote_ref, rev_range) = range;

    let count_output = git_output(project_root, &["rev-list", "--count", &rev_range])?;
    if !count_output.status.success() {
        return Ok(None);
    }
    let commits_ahead = String::from_utf8_lossy(&count_output.stdout)
        .trim()
        .parse::<u64>()
        .unwrap_or(0);
    if commits_ahead == 0 {
        return Ok(None);
    }

    let log_output = git_output(project_root, &["log", "--format=%H%x09%s", &rev_range])?;
    if !log_output.status.success() {
        return Ok(None);
    }

    let commits = String::from_utf8_lossy(&log_output.stdout)
        .lines()
        .filter_map(|line| {
            let (sha, subject) = line.split_once('\t')?;
            Some(UnpushedCommitRecord {
                sha: sha.to_string(),
                subject: subject.to_string(),
            })
        })
        .collect::<Vec<_>>();
    if commits.is_empty() {
        return Ok(None);
    }

    Ok(Some(UnpushedCommitsSidecar {
        branch: branch.to_string(),
        remote_ref,
        commits_ahead,
        commits,
        recovery_command: format_git_push_recovery_command(branch),
    }))
}

#[rustfmt::skip]
pub(super) fn persist_unpushed_commits_sidecar(project_root: &Path, session: &MetaSessionState, session_dir: &Path) -> Result<Option<ArtifactRollbackGuard>> {
    if session.resolved_identity().vcs_kind != VcsKind::Git { return Ok(None); }
    let Some(branch) = session.branch.as_deref() else { return Ok(None); };
    let Some(sidecar) = inspect_unpushed_commits(project_root, branch)? else { return Ok(None); };
    fs::create_dir_all(session_dir.join("output"))?;
    let sidecar_path = session_dir.join(UNPUSHED_COMMITS_SIDECAR_PATH);
    let sidecar_contents = serde_json::to_vec_pretty(&sidecar)?;
    let rollback_guard = artifact_rollback_guard(&sidecar_path, sidecar_contents.as_slice())?;
    write_sidecar_atomically(&sidecar_path, &sidecar_contents)?;
    Ok(rollback_guard)
}

#[rustfmt::skip]
pub(super) fn persist_fix_finding_recovery_sidecar(project_root: &Path, session: &MetaSessionState, session_dir: &Path) -> Result<Option<ArtifactRollbackGuard>> {
    let Some(sidecar) = crate::session_fix_finding_recovery::build_recovery_sidecar(project_root, session) else { return Ok(None); };
    fs::create_dir_all(session_dir.join("output"))?;
    let sidecar_path = crate::session_fix_finding_recovery::recovery_sidecar_path(session_dir);
    let sidecar_contents = serde_json::to_vec_pretty(&sidecar)?;
    let rollback_guard = artifact_rollback_guard(&sidecar_path, sidecar_contents.as_slice())?;
    write_sidecar_atomically(&sidecar_path, &sidecar_contents)?;
    Ok(rollback_guard)
}

#[rustfmt::skip]
fn rollback_sidecar(rollback_guard: &ArtifactRollbackGuard) -> std::io::Result<()> {
    match &rollback_guard.rollback_action {
        ArtifactRollbackAction::RemoveIfContentsMatch => super::remove_artifact_if_unchanged(&rollback_guard.artifact_path, rollback_guard.expected_contents.as_slice(), super::ArtifactRollbackLabels { removed_cleanup: "removed_recovery_sidecar", missing_after_match_cleanup: "sidecar_missing_after_match", remove_failed_cleanup: "sidecar_remove_failed", preserved_cleanup: "preexisting_sidecar_preserved", missing_cleanup: "sidecar_missing", read_failed_cleanup: "sidecar_read_failed", artifact_label: "recovery sidecar" }),
        ArtifactRollbackAction::RestoreOriginal(original_contents) => match fs::read(&rollback_guard.artifact_path) {
            Ok(current_contents) if current_contents == rollback_guard.expected_contents => { fs::write(&rollback_guard.artifact_path, original_contents)?; warn!(artifact_path = %rollback_guard.artifact_path.display(), rollback_cleanup = "restored_preexisting_recovery_sidecar", "Rollback restored preexisting recovery sidecar after reconciliation failure"); Ok(()) }
            Ok(_) => { warn!(artifact_path = %rollback_guard.artifact_path.display(), rollback_cleanup = "preexisting_sidecar_preserved", "Rollback preserved recovery sidecar because contents changed after reconciliation failure"); Ok(()) }
            Err(err) if err.kind() == ErrorKind::NotFound => { warn!(artifact_path = %rollback_guard.artifact_path.display(), rollback_cleanup = "sidecar_missing", "Rollback found no recovery sidecar to restore after reconciliation failure"); Ok(()) }
            Err(err) => { warn!(artifact_path = %rollback_guard.artifact_path.display(), rollback_cleanup = "sidecar_read_failed", error = %err, "Rollback failed to read recovery sidecar for content-aware restore after reconciliation failure"); Ok(()) }
        },
    }
}

#[rustfmt::skip]
pub(super) fn rollback_sidecars(rollback_guards: &[ArtifactRollbackGuard]) -> std::io::Result<()> { for rollback_guard in rollback_guards { rollback_sidecar(rollback_guard)?; } Ok(()) }

#[rustfmt::skip]
fn artifact_rollback_guard(artifact_path: &Path, expected_contents: &[u8]) -> std::io::Result<Option<ArtifactRollbackGuard>> {
    match fs::read(artifact_path) {
        Ok(current_contents) if current_contents == expected_contents => Ok(None),
        Ok(current_contents) => Ok(Some(ArtifactRollbackGuard {
            artifact_path: artifact_path.to_path_buf(),
            expected_contents: expected_contents.to_vec(),
            rollback_action: ArtifactRollbackAction::RestoreOriginal(current_contents),
        })),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(Some(ArtifactRollbackGuard {
            artifact_path: artifact_path.to_path_buf(),
            expected_contents: expected_contents.to_vec(),
            rollback_action: ArtifactRollbackAction::RemoveIfContentsMatch,
        })),
        Err(err) => Err(err),
    }
}

#[rustfmt::skip]
pub(super) fn write_sidecar_atomically(sidecar_path: &Path, contents: &[u8]) -> Result<()> {
    let sidecar_dir = sidecar_path.parent().ok_or_else(|| anyhow!("Recovery sidecar path has no parent: {}", sidecar_path.display()))?;
    let mut temp_file = tempfile::NamedTempFile::new_in(sidecar_dir).with_context(|| format!("Failed to create temporary recovery sidecar in {}", sidecar_dir.display()))?;
    temp_file.as_file_mut().write_all(contents).with_context(|| format!("Failed to write temporary recovery sidecar for {}", sidecar_path.display()))?;
    temp_file.as_file_mut().sync_all().with_context(|| format!("Failed to sync temporary recovery sidecar for {}", sidecar_path.display()))?;
    preserve_existing_permissions_if_present(temp_file.as_file_mut(), sidecar_path, "recovery sidecar")?;
    temp_file.persist(sidecar_path).map_err(|err| anyhow!("Failed to publish recovery sidecar {}: {}", sidecar_path.display(), err.error))?;
    Ok(())
}

#[rustfmt::skip]
fn format_git_push_recovery_command(branch: &str) -> String {
    if branch_is_shell_word_safe(branch) {
        format!("git push -u origin {branch}")
    } else {
        format!("git push -u origin {}", shell_escape_for_command(branch))
    }
}

#[rustfmt::skip]
fn branch_is_shell_word_safe(branch: &str) -> bool { branch.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-')) }
