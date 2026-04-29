//! Sidecar jj snapshot journaling.
//!
//! This module intentionally does not participate in canonical repository
//! operations. Git remains the canonical backend; jj is only used as an
//! optional sidecar journal.

use csa_core::vcs::{JournalError, RevisionId, SnapshotJournal};
use csa_lock::acquire_project_resource_lock;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const SESSION_DIR_ENV: &str = "CSA_SESSION_DIR";
const STATE_FILE_NAME: &str = "jj-journal-state.json";
const MAX_SNAPSHOT_MESSAGE_LEN: usize = 4096;
const SNAPSHOT_REVISION_ARGS: [&str; 6] = ["log", "--no-graph", "-r", "@-", "-T", "change_id"];
const CURRENT_OPERATION_ARGS: [&str; 9] = [
    "op",
    "log",
    "--ignore-working-copy",
    "--at-op=@",
    "--no-graph",
    "-n",
    "1",
    "-T",
    "self.id().short(12)",
];

#[derive(Debug, Clone)]
pub struct JjJournal {
    project_root: PathBuf,
    state_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
struct JournalState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project_root: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_start_revision: Option<RevisionId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    snapshot_revisions: Vec<RevisionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_start_operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_operation_id: Option<String>,
}

impl JjJournal {
    pub fn new(project_root: impl AsRef<Path>) -> Result<Self, JournalError> {
        let project_root = absolutize_project_root(project_root.as_ref())?;
        let state_path = derive_state_path()?;
        Ok(Self {
            project_root,
            state_path,
        })
    }

    pub fn with_session_dir(
        project_root: impl AsRef<Path>,
        session_dir: impl AsRef<Path>,
    ) -> Result<Self, JournalError> {
        let project_root = absolutize_project_root(project_root.as_ref())?;
        Ok(Self {
            project_root,
            state_path: session_dir.as_ref().join(STATE_FILE_NAME),
        })
    }

    #[cfg(test)]
    fn with_state_path(project_root: impl AsRef<Path>, state_path: impl AsRef<Path>) -> Self {
        Self {
            project_root: std::path::absolute(project_root.as_ref())
                .unwrap_or_else(|_| project_root.as_ref().to_path_buf()),
            state_path: state_path.as_ref().to_path_buf(),
        }
    }

    fn capture_snapshot_revision(
        &self,
        sanitized_message: &str,
    ) -> Result<RevisionId, JournalError> {
        let desc_output = self.run_jj(["log", "--no-graph", "-r", "@", "-T", "description"])?;
        let preserved_description = String::from_utf8_lossy(&desc_output.stdout).to_string();

        self.run_jj(["describe", "-m", sanitized_message])?;
        self.run_jj(["new", "-m", preserved_description.as_str()])?;

        let output = self.run_jj(SNAPSHOT_REVISION_ARGS)?;
        let revision = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if revision.is_empty() {
            return Err(JournalError::CommandFailed {
                command: format_jj_command(SNAPSHOT_REVISION_ARGS),
                message: "snapshot revision id was empty".to_string(),
            });
        }
        Ok(RevisionId::from(revision))
    }

    fn run_jj<I, S>(&self, args: I) -> Result<std::process::Output, JournalError>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let collected = collect_jj_args(args);
        let output = Command::new("jj")
            .args(&collected)
            .current_dir(&self.project_root)
            .output()
            .map_err(|err| match err.kind() {
                std::io::ErrorKind::NotFound => JournalError::Unavailable(
                    "jj binary not found; git fallback is intentionally disabled".to_string(),
                ),
                _ => JournalError::Io(format!("failed to run jj: {err}")),
            })?;

        if !output.status.success() {
            let command = format_jj_command_from_collected(&collected);
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let message = if stderr.is_empty() {
                format!("exit status {}", output.status)
            } else {
                stderr
            };
            return Err(JournalError::CommandFailed { command, message });
        }

        Ok(output)
    }

    fn current_operation_id(&self) -> Result<String, JournalError> {
        let output = self.run_jj(CURRENT_OPERATION_ARGS)?;
        let op_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if op_id.is_empty() {
            return Err(JournalError::CommandFailed {
                command: format_jj_command(CURRENT_OPERATION_ARGS),
                message: "operation id was empty".to_string(),
            });
        }
        Ok(op_id)
    }

    pub fn snapshot_revisions(&self) -> Result<Vec<RevisionId>, JournalError> {
        Ok(self
            .read_state()?
            .map(|state| state.snapshot_revisions)
            .unwrap_or_default())
    }

    pub fn aggregate_session(
        &self,
        session_id: &str,
        snapshot_count: usize,
        message_template: &str,
    ) -> Result<(), JournalError> {
        let _lock = acquire_project_resource_lock(
            &self.project_root,
            "jj-journal",
            "aggregate",
            &format!("aggregate session {session_id}"),
        )
        .map_err(|err| JournalError::CommandFailed {
            command: "acquire_project_resource_lock(jj-journal/aggregate)".to_string(),
            message: err.to_string(),
        })?;

        let mut state = self.read_state()?.unwrap_or_default();
        validate_state_project_root(&state, &self.project_root)?;
        if snapshot_count == 0 || state.snapshot_revisions.is_empty() {
            return Ok(());
        }
        if state.snapshot_revisions.len() != snapshot_count {
            return Err(JournalError::InvalidState(format!(
                "recorded {} jj snapshots for session {session_id}, expected {snapshot_count}",
                state.snapshot_revisions.len()
            )));
        }

        let pre_squash_op = self.current_operation_id()?;
        let message = format_aggregate_message(message_template, session_id, snapshot_count)?;
        self.combine_snapshot_revisions(&state.snapshot_revisions, &message)?;
        match self.run_jj(["git", "export"]) {
            Ok(_) => {
                state.snapshot_revisions.clear();
                state.session_start_revision = None;
                state.session_start_operation_id = None;
                state.last_operation_id = None;
                self.write_state(&state)?;
                Ok(())
            }
            Err(export_err) => {
                let restore = self.run_jj(["op", "restore", pre_squash_op.as_str()]);
                match restore {
                    Ok(_) => Err(export_err),
                    Err(restore_err) => Err(JournalError::CommandFailed {
                        command: "jj op restore <pre-squash-op>".to_string(),
                        message: format!(
                            "jj git export failed ({export_err}); rollback also failed ({restore_err})"
                        ),
                    }),
                }
            }
        }
    }

    fn combine_snapshot_revisions(
        &self,
        revisions: &[RevisionId],
        message: &str,
    ) -> Result<(), JournalError> {
        let Some(first) = revisions.first() else {
            return Ok(());
        };
        if revisions.len() == 1 {
            self.run_jj(["describe", "-r", first.as_str(), "-m", message])?;
            return Ok(());
        }

        let mut args = vec![OsString::from("squash")];
        for revision in &revisions[1..] {
            args.push(OsString::from("--from"));
            args.push(OsString::from(revision.as_str()));
        }
        args.push(OsString::from("--into"));
        args.push(OsString::from(first.as_str()));
        args.push(OsString::from("-m"));
        args.push(OsString::from(message));
        self.run_jj(args)?;
        Ok(())
    }

    fn read_state(&self) -> Result<Option<JournalState>, JournalError> {
        match fs::read_to_string(&self.state_path) {
            Ok(raw) => {
                let state = serde_json::from_str::<JournalState>(&raw)
                    .map_err(|err| JournalError::InvalidState(err.to_string()))?;
                Ok(Some(state))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    fn write_state(&self, state: &JournalState) -> Result<(), JournalError> {
        write_state_atomically(&self.state_path, state)
    }

    fn snapshot_with_revision_supplier<F>(
        &self,
        message: &str,
        revision_supplier: F,
    ) -> Result<RevisionId, JournalError>
    where
        F: FnOnce(&str) -> Result<RevisionId, JournalError>,
    {
        let sanitized = sanitize_snapshot_message(message)?;
        let truncated_reason: String = sanitized.chars().take(64).collect();
        let reason = format!("snapshot: {truncated_reason}");
        let _lock =
            acquire_project_resource_lock(&self.project_root, "jj-journal", "snapshot", &reason)
                .map_err(|err| JournalError::CommandFailed {
                    command: "acquire_project_resource_lock(jj-journal/snapshot)".to_string(),
                    message: err.to_string(),
                })?;

        let mut current_state = self.read_state()?.unwrap_or_default();
        validate_state_project_root(&current_state, &self.project_root)?;
        let operation_before_snapshot = self.current_operation_id()?;
        if let Some(expected_op_id) = current_state.last_operation_id.as_deref()
            && expected_op_id != operation_before_snapshot
        {
            return Err(JournalError::InvalidState(format!(
                "jj operation drift detected for {}: expected last operation {}, found {}; refusing sidecar snapshot",
                self.project_root.display(),
                expected_op_id,
                operation_before_snapshot
            )));
        }
        let revision = revision_supplier(&sanitized)?;
        let operation_after_snapshot = self.current_operation_id()?;

        if current_state.project_root.is_none() {
            current_state.project_root = Some(self.project_root.clone());
        }
        if current_state.session_start_revision.is_none() {
            current_state.session_start_revision = Some(revision.clone());
            current_state.session_start_operation_id = Some(operation_before_snapshot);
        }
        current_state.snapshot_revisions.push(revision.clone());
        current_state.last_operation_id = Some(operation_after_snapshot);
        self.write_state(&current_state)?;

        Ok(revision)
    }

    #[cfg(test)]
    fn write_state_with_fault_injection(
        &self,
        state: &JournalState,
        fail_after_write: bool,
    ) -> Result<(), JournalError> {
        write_state_atomically_inner(&self.state_path, state, fail_after_write)
    }
}

impl SnapshotJournal for JjJournal {
    fn snapshot(&self, message: &str) -> Result<RevisionId, JournalError> {
        self.snapshot_with_revision_supplier(message, |msg| self.capture_snapshot_revision(msg))
    }

    fn session_start_revision(&self) -> Result<Option<RevisionId>, JournalError> {
        Ok(self
            .read_state()?
            .and_then(|state| state.session_start_revision))
    }
}

fn collect_jj_args<I, S>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut collected: Vec<OsString> = vec!["--no-pager".into(), "--color=never".into()];
    collected.extend(args.into_iter().map(Into::into));
    collected
}

fn format_jj_command<I, S>(args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let collected = collect_jj_args(args);
    format_jj_command_from_collected(&collected)
}

fn format_jj_command_from_collected(collected: &[OsString]) -> String {
    format!(
        "jj {}",
        collected
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    )
}

fn sanitize_snapshot_message(message: &str) -> Result<String, JournalError> {
    if message.contains('\0') {
        return Err(JournalError::InvalidMessage(
            "message contains null byte".to_string(),
        ));
    }

    let normalized = message.replace("\r\n", "\n").replace('\r', "\n");
    let collapsed = normalized
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" / ");

    let bounded: String = collapsed.chars().take(MAX_SNAPSHOT_MESSAGE_LEN).collect();
    if bounded.is_empty() {
        return Err(JournalError::InvalidMessage(
            "message empty after sanitization".to_string(),
        ));
    }
    Ok(bounded)
}

fn format_aggregate_message(
    template: &str,
    session_id: &str,
    snapshot_count: usize,
) -> Result<String, JournalError> {
    let message = template
        .replace("{session_id}", session_id)
        .replace("{count}", &snapshot_count.to_string());
    if message.contains('\0') {
        return Err(JournalError::InvalidMessage(
            "aggregate message contains null byte".to_string(),
        ));
    }
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return Err(JournalError::InvalidMessage(
            "aggregate message empty after template expansion".to_string(),
        ));
    }
    Ok(trimmed.to_string())
}

fn absolutize_project_root(project_root: &Path) -> Result<PathBuf, JournalError> {
    std::path::absolute(project_root)
        .map_err(|err| JournalError::Io(format!("failed to resolve project root: {err}")))
}

fn derive_state_path() -> Result<PathBuf, JournalError> {
    std::env::var_os(SESSION_DIR_ENV)
        .map(|session_dir| PathBuf::from(session_dir).join(STATE_FILE_NAME))
        .ok_or_else(|| {
            JournalError::Unavailable(
                "CSA_SESSION_DIR not set; sidecar jj snapshot journal requires CSA session context"
                    .to_string(),
            )
        })
}

fn validate_state_project_root(
    state: &JournalState,
    project_root: &Path,
) -> Result<(), JournalError> {
    if let Some(recorded_root) = state.project_root.as_ref()
        && recorded_root != project_root
    {
        return Err(JournalError::InvalidState(format!(
            "journal state belongs to {}, not {}; refusing sidecar snapshot",
            recorded_root.display(),
            project_root.display()
        )));
    }
    Ok(())
}

fn temp_state_path(path: &Path) -> Result<PathBuf, JournalError> {
    let file_name = path.file_name().ok_or_else(|| {
        JournalError::InvalidState(format!("state path has no file name: {}", path.display()))
    })?;
    let mut tmp_name = file_name.to_os_string();
    tmp_name.push(".tmp");
    Ok(path.with_file_name(tmp_name))
}

fn write_state_atomically(path: &Path, state: &JournalState) -> Result<(), JournalError> {
    write_state_atomically_inner(path, state, false)
}

fn write_state_atomically_inner(
    path: &Path,
    state: &JournalState,
    fail_after_write: bool,
) -> Result<(), JournalError> {
    let parent = path.parent().ok_or_else(|| {
        JournalError::InvalidState(format!("state path has no parent: {}", path.display()))
    })?;
    fs::create_dir_all(parent)?;
    let tmp_path = temp_state_path(path)?;
    let encoded = serde_json::to_vec_pretty(state)
        .map_err(|err| JournalError::InvalidState(err.to_string()))?;

    let mut tmp_file = fs::File::create(&tmp_path)?;
    tmp_file.write_all(&encoded)?;
    tmp_file.sync_all()?;
    drop(tmp_file);

    if fail_after_write {
        return Err(JournalError::Io(
            "simulated crash after temp write".to_string(),
        ));
    }

    fs::rename(&tmp_path, path)?;
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

#[cfg(test)]
#[path = "jj_journal_tests.rs"]
mod jj_journal_tests;
