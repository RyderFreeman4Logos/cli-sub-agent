//! Session checkpoints for mid-flight observability and git-note audit trails.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Checkpoint metadata stored under `<session_dir>/checkpoints/*.toml`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Checkpoint {
    pub phase: String,
    pub summary: String,
    pub timestamp: DateTime<Utc>,
    pub sequence: u32,
}

/// The git notes ref namespace for CSA checkpoints.
const NOTES_REF: &str = "refs/notes/csa-checkpoints";

const CHECKPOINTS_DIR_NAME: &str = "checkpoints";

/// Emit a checkpoint file to the session's checkpoints directory.
pub fn emit_checkpoint(session_dir: &Path, phase: &str, summary: &str) -> Result<PathBuf> {
    let checkpoints_dir = checkpoints_dir(session_dir);
    fs::create_dir_all(&checkpoints_dir).with_context(|| {
        format!(
            "Failed to create checkpoints directory '{}'",
            checkpoints_dir.display()
        )
    })?;

    let sequence = next_checkpoint_sequence(&checkpoints_dir)?;
    let checkpoint = Checkpoint {
        phase: phase.to_string(),
        summary: summary.to_string(),
        timestamp: Utc::now(),
        sequence,
    };

    let path = checkpoint_path(&checkpoints_dir, sequence);
    let tmp_path = path.with_extension("toml.tmp");
    let body = toml::to_string_pretty(&checkpoint).context("Failed to serialize checkpoint")?;
    fs::write(&tmp_path, body)
        .with_context(|| format!("Failed to write checkpoint '{}'", tmp_path.display()))?;
    fs::rename(&tmp_path, &path)
        .with_context(|| format!("Failed to move checkpoint '{}' into place", path.display()))?;

    Ok(path)
}

/// Read the latest checkpoint from a session's checkpoints directory.
pub fn read_latest_checkpoint(session_dir: &Path) -> Result<Option<Checkpoint>> {
    let mut checkpoints = read_checkpoints(session_dir)?;
    Ok(checkpoints.pop())
}

/// Read all checkpoints in sequence order.
pub fn read_checkpoints(session_dir: &Path) -> Result<Vec<Checkpoint>> {
    let checkpoints_dir = checkpoints_dir(session_dir);
    if !checkpoints_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    for entry in fs::read_dir(&checkpoints_dir).with_context(|| {
        format!(
            "Failed to read checkpoints directory '{}'",
            checkpoints_dir.display()
        )
    })? {
        let entry = entry?;
        let path = entry.path();
        let Some(sequence) = checkpoint_sequence_from_path(&path) else {
            continue;
        };
        entries.push((sequence, path));
    }
    entries.sort_by_key(|(sequence, _)| *sequence);

    entries
        .into_iter()
        .map(|(_, path)| read_checkpoint_file(&path))
        .collect()
}

fn checkpoints_dir(session_dir: &Path) -> PathBuf {
    session_dir.join(CHECKPOINTS_DIR_NAME)
}

fn checkpoint_path(checkpoints_dir: &Path, sequence: u32) -> PathBuf {
    checkpoints_dir.join(format!("{sequence:04}.toml"))
}

fn next_checkpoint_sequence(checkpoints_dir: &Path) -> Result<u32> {
    let mut max_sequence = 0;
    for entry in fs::read_dir(checkpoints_dir).with_context(|| {
        format!(
            "Failed to read checkpoints directory '{}'",
            checkpoints_dir.display()
        )
    })? {
        let entry = entry?;
        let path = entry.path();
        if let Some(sequence) = checkpoint_sequence_from_path(&path) {
            max_sequence = max_sequence.max(sequence);
        }
    }
    Ok(max_sequence + 1)
}

fn checkpoint_sequence_from_path(path: &Path) -> Option<u32> {
    let file_name = path.file_name()?.to_str()?;
    let number = file_name.strip_suffix(".toml")?;
    if number.len() != 4 || !number.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    number.parse().ok()
}

fn read_checkpoint_file(path: &Path) -> Result<Checkpoint> {
    let body = fs::read_to_string(path)
        .with_context(|| format!("Failed to read checkpoint '{}'", path.display()))?;
    toml::from_str(&body)
        .with_context(|| format!("Failed to parse checkpoint '{}'", path.display()))
}

/// Checkpoint metadata written as a git note.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct CheckpointNote {
    pub session_id: String,
    pub tool: Option<String>,
    pub status: String,
    pub created_at: String,
    pub completed_at: String,
    pub turn_count: u32,
    pub token_usage: Option<TokenUsageSummary>,
    pub description: Option<String>,
    /// jj operation log ID at checkpoint time (for `jj op restore` rollback).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op_id: Option<String>,
}

/// Summary of token usage for the checkpoint note.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct TokenUsageSummary {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Write a checkpoint note for a session.
///
/// Finds the most recent commit that modified this session's directory
/// and attaches a TOML-formatted git note to it.
/// If no commits exist for the session, returns an error.
pub fn write_checkpoint_note(sessions_dir: &Path, note: &CheckpointNote) -> Result<()> {
    crate::git::ensure_git_init(sessions_dir)?;

    let target_commit = find_session_commit(sessions_dir, &note.session_id)?;

    let toml_body =
        toml::to_string_pretty(note).context("Failed to serialize checkpoint note to TOML")?;

    // Write note: git notes --ref=<ref> add -f -m "<body>" <commit>
    // -f (force) overwrites any existing note on this commit
    let output = Command::new("git")
        .args([
            "notes",
            &format!("--ref={NOTES_REF}"),
            "add",
            "-f",
            "-m",
            &toml_body,
            &target_commit,
        ])
        .current_dir(sessions_dir)
        .output()
        .context("Failed to run git notes add")?;

    if !output.status.success() {
        anyhow::bail!(
            "git notes add failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    tracing::info!(
        session_id = %note.session_id,
        commit = %target_commit,
        "Checkpoint note written"
    );

    Ok(())
}

/// Find the most recent commit that modified a session's directory.
fn find_session_commit(sessions_dir: &Path, session_id: &str) -> Result<String> {
    let session_path = format!("{session_id}/");

    let output = Command::new("git")
        .args(["log", "-1", "--format=%H", "--", &session_path])
        .current_dir(sessions_dir)
        .output()
        .context("Failed to find session commit")?;

    if !output.status.success() {
        anyhow::bail!(
            "git log failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if sha.is_empty() {
        anyhow::bail!("No commits found for session '{session_id}' — commit the session first");
    }

    Ok(sha)
}

/// Read the checkpoint note attached to a specific commit.
pub fn read_checkpoint_note(sessions_dir: &Path, commit: &str) -> Result<Option<CheckpointNote>> {
    if !sessions_dir.join(".git").exists() {
        anyhow::bail!("No git repository in sessions directory");
    }

    let output = Command::new("git")
        .args(["notes", &format!("--ref={NOTES_REF}"), "show", commit])
        .current_dir(sessions_dir)
        .output()
        .context("Failed to run git notes show")?;

    if !output.status.success() {
        // git notes show exits non-zero when no note exists — not an error
        return Ok(None);
    }

    let body = String::from_utf8_lossy(&output.stdout);
    let note: CheckpointNote =
        toml::from_str(&body).context("Failed to parse checkpoint note TOML")?;
    Ok(Some(note))
}

/// List all checkpoint notes in the sessions repo.
/// Returns a list of (commit_hash, CheckpointNote) pairs.
pub fn list_checkpoint_notes(sessions_dir: &Path) -> Result<Vec<(String, CheckpointNote)>> {
    if !sessions_dir.join(".git").exists() {
        return Ok(Vec::new());
    }

    // git log --format="%H" <ref> lists all commits that have notes
    let output = Command::new("git")
        .args(["notes", &format!("--ref={NOTES_REF}"), "list"])
        .current_dir(sessions_dir)
        .output()
        .context("Failed to run git notes list")?;

    if !output.status.success() {
        // No notes ref yet — empty list
        return Ok(Vec::new());
    }

    let mut results = Vec::new();
    let list_output = String::from_utf8_lossy(&output.stdout);

    for line in list_output.lines() {
        // Format: "<note_blob_sha> <annotated_commit_sha>"
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let commit_sha = parts[1];
            match read_checkpoint_note(sessions_dir, commit_sha) {
                Ok(Some(note)) => results.push((commit_sha.to_string(), note)),
                Ok(None) => {} // note disappeared between list and show — benign race
                Err(e) => {
                    tracing::warn!(
                        commit = commit_sha,
                        error = %e,
                        "Skipping corrupted checkpoint note"
                    );
                }
            }
        }
    }

    Ok(results)
}

/// Build a CheckpointNote from a loaded session state.
pub fn note_from_session(session: &crate::MetaSessionState) -> CheckpointNote {
    // Deterministic: pick the lexicographically first tool name
    let tool = {
        let mut keys: Vec<&String> = session.tools.keys().collect();
        keys.sort();
        keys.first().map(|k| (*k).clone())
    };

    let token_usage = session
        .total_token_usage
        .as_ref()
        .map(|u| TokenUsageSummary {
            input_tokens: u.input_tokens.unwrap_or(0),
            output_tokens: u.output_tokens.unwrap_or(0),
        });

    let op_id = session.resolved_identity().op_id;

    CheckpointNote {
        session_id: session.meta_session_id.clone(),
        tool,
        status: format!("{:?}", session.phase),
        created_at: session.created_at.to_rfc3339(),
        completed_at: session.last_accessed.to_rfc3339(),
        turn_count: session.turn_count,
        token_usage,
        description: session.description.clone(),
        op_id,
    }
}

#[cfg(test)]
#[path = "checkpoint_tests.rs"]
mod tests;
