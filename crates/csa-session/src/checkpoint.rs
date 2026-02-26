//! Git Notes checkpoint for session audit trail.
//!
//! Writes structured TOML metadata to `refs/notes/csa-checkpoints` for a session.
//! This provides a lightweight, git-native audit trail without modifying session files.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// The git notes ref namespace for CSA checkpoints.
const NOTES_REF: &str = "refs/notes/csa-checkpoints";

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
pub fn write_checkpoint(sessions_dir: &Path, note: &CheckpointNote) -> Result<()> {
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
    let session_path = format!("{}/", session_id);

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
        anyhow::bail!(
            "No commits found for session '{}' — commit the session first",
            session_id
        );
    }

    Ok(sha)
}

/// Read the checkpoint note attached to a specific commit.
pub fn read_checkpoint(sessions_dir: &Path, commit: &str) -> Result<Option<CheckpointNote>> {
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
pub fn list_checkpoints(sessions_dir: &Path) -> Result<Vec<(String, CheckpointNote)>> {
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
            match read_checkpoint(sessions_dir, commit_sha) {
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

    CheckpointNote {
        session_id: session.meta_session_id.clone(),
        tool,
        status: format!("{:?}", session.phase),
        created_at: session.created_at.to_rfc3339(),
        completed_at: session.last_accessed.to_rfc3339(),
        turn_count: session.turn_count,
        token_usage,
        description: session.description.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_note() -> CheckpointNote {
        CheckpointNote {
            session_id: "01TESTID000000000000000000".to_string(),
            tool: Some("codex".to_string()),
            status: "Completed".to_string(),
            created_at: "2026-02-13T10:00:00+00:00".to_string(),
            completed_at: "2026-02-13T10:05:00+00:00".to_string(),
            turn_count: 3,
            token_usage: Some(TokenUsageSummary {
                input_tokens: 5000,
                output_tokens: 1200,
            }),
            description: Some("Test session".to_string()),
        }
    }

    #[test]
    fn test_checkpoint_note_toml_roundtrip() {
        let note = make_note();
        let toml_str = toml::to_string_pretty(&note).unwrap();
        let parsed: CheckpointNote = toml::from_str(&toml_str).unwrap();
        assert_eq!(note, parsed);
    }

    #[test]
    fn test_checkpoint_note_toml_format() {
        let note = make_note();
        let toml_str = toml::to_string_pretty(&note).unwrap();
        assert!(toml_str.contains("session_id = \"01TESTID000000000000000000\""));
        assert!(toml_str.contains("tool = \"codex\""));
        assert!(toml_str.contains("turn_count = 3"));
        assert!(toml_str.contains("input_tokens = 5000"));
    }

    #[test]
    fn test_checkpoint_note_without_optional_fields() {
        let note = CheckpointNote {
            session_id: "01TESTID000000000000000000".to_string(),
            tool: None,
            status: "Running".to_string(),
            created_at: "2026-02-13T10:00:00+00:00".to_string(),
            completed_at: "2026-02-13T10:00:00+00:00".to_string(),
            turn_count: 0,
            token_usage: None,
            description: None,
        };
        let toml_str = toml::to_string_pretty(&note).unwrap();
        let parsed: CheckpointNote = toml::from_str(&toml_str).unwrap();
        assert_eq!(note, parsed);
    }

    #[test]
    fn test_write_checkpoint_no_commits_errors() {
        let tmp = tempdir().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        crate::git::ensure_git_init(&sessions_dir).unwrap();

        let note = make_note();
        let result = write_checkpoint(&sessions_dir, &note);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("commit") || err_msg.contains("No commits"),
            "Should mention commits, got: {err_msg}"
        );
    }

    #[test]
    fn test_checkpoint_targets_session_commit_not_head() {
        let tmp = tempdir().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        crate::git::ensure_git_init(&sessions_dir).unwrap();

        // Create session A and commit
        let session_a = ulid::Ulid::new().to_string();
        let dir_a = sessions_dir.join(&session_a);
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::write(dir_a.join("state.toml"), "a = true").unwrap();
        crate::git::commit_session(&sessions_dir, &session_a, "session A").unwrap();

        let commit_a = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&sessions_dir)
            .output()
            .unwrap();
        let commit_a_sha = String::from_utf8_lossy(&commit_a.stdout).trim().to_string();

        // Create session B and commit (now HEAD moves forward)
        let session_b = ulid::Ulid::new().to_string();
        let dir_b = sessions_dir.join(&session_b);
        std::fs::create_dir_all(&dir_b).unwrap();
        std::fs::write(dir_b.join("state.toml"), "b = true").unwrap();
        crate::git::commit_session(&sessions_dir, &session_b, "session B").unwrap();

        // Write checkpoint for session A (should target commit_a, NOT HEAD)
        let mut note = make_note();
        note.session_id.clone_from(&session_a);
        write_checkpoint(&sessions_dir, &note).unwrap();

        // Verify note is on commit_a, not HEAD
        let read_back = read_checkpoint(&sessions_dir, &commit_a_sha).unwrap();
        assert!(read_back.is_some(), "Note should be on session A's commit");
        assert_eq!(read_back.unwrap().session_id, session_a);
    }

    #[test]
    fn test_note_from_session_deterministic_tool_selection() {
        let session = crate::MetaSessionState {
            meta_session_id: "01TEST".to_string(),
            description: None,
            project_path: "/tmp".to_string(),
            branch: None,
            created_at: chrono::Utc::now(),
            last_accessed: chrono::Utc::now(),
            genealogy: Default::default(),
            tools: {
                let mut m = std::collections::HashMap::new();
                m.insert(
                    "gemini-cli".to_string(),
                    crate::state::ToolState {
                        provider_session_id: None,
                        last_action_summary: String::new(),
                        last_exit_code: 0,
                        token_usage: None,
                        updated_at: chrono::Utc::now(),
                    },
                );
                m.insert(
                    "codex".to_string(),
                    crate::state::ToolState {
                        provider_session_id: None,
                        last_action_summary: String::new(),
                        last_exit_code: 0,
                        token_usage: None,
                        updated_at: chrono::Utc::now(),
                    },
                );
                m
            },
            context_status: Default::default(),
            total_token_usage: None,
            phase: crate::state::SessionPhase::Active,
            task_context: Default::default(),
            turn_count: 0,
            token_budget: None,
            sandbox_info: None,

            termination_reason: None,
            is_seed_candidate: false,
            git_head_at_creation: None,
            last_return_packet: None,
            fork_call_timestamps: Vec::new(),
        };

        // Should deterministically pick "codex" (alphabetically first)
        let note = note_from_session(&session);
        assert_eq!(note.tool, Some("codex".to_string()));
    }

    #[test]
    fn test_write_and_read_checkpoint_roundtrip() {
        let tmp = tempdir().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        crate::git::ensure_git_init(&sessions_dir).unwrap();

        // Create a commit to attach the note to
        let session_id = ulid::Ulid::new().to_string();
        let session_dir = sessions_dir.join(&session_id);
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(session_dir.join("state.toml"), "test = true").unwrap();
        crate::git::commit_session(&sessions_dir, &session_id, "test session").unwrap();

        // Get the commit SHA for this session
        let head = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&sessions_dir)
            .output()
            .unwrap();
        let head_sha = String::from_utf8_lossy(&head.stdout).trim().to_string();

        // Write checkpoint — note session_id must match the committed session
        let mut note = make_note();
        note.session_id.clone_from(&session_id);
        write_checkpoint(&sessions_dir, &note).unwrap();

        // Read it back
        let read_back = read_checkpoint(&sessions_dir, &head_sha).unwrap();
        assert!(read_back.is_some());
        assert_eq!(read_back.unwrap(), note);
    }

    #[test]
    fn test_read_checkpoint_no_note_returns_none() {
        let tmp = tempdir().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        crate::git::ensure_git_init(&sessions_dir).unwrap();

        // Create a commit without a note
        let session_id = ulid::Ulid::new().to_string();
        let session_dir = sessions_dir.join(&session_id);
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(session_dir.join("state.toml"), "test = true").unwrap();
        crate::git::commit_session(&sessions_dir, &session_id, "test").unwrap();

        let head = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&sessions_dir)
            .output()
            .unwrap();
        let head_sha = String::from_utf8_lossy(&head.stdout).trim().to_string();

        let result = read_checkpoint(&sessions_dir, &head_sha).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_list_checkpoints_empty_repo() {
        let tmp = tempdir().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        crate::git::ensure_git_init(&sessions_dir).unwrap();

        let results = list_checkpoints(&sessions_dir).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_list_checkpoints_with_notes() {
        let tmp = tempdir().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        crate::git::ensure_git_init(&sessions_dir).unwrap();

        // Create commit + checkpoint
        let session_id = ulid::Ulid::new().to_string();
        let session_dir = sessions_dir.join(&session_id);
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(session_dir.join("state.toml"), "test = true").unwrap();
        crate::git::commit_session(&sessions_dir, &session_id, "test").unwrap();

        let mut note = make_note();
        note.session_id.clone_from(&session_id);
        write_checkpoint(&sessions_dir, &note).unwrap();

        let results = list_checkpoints(&sessions_dir).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1.session_id, session_id);
    }

    #[test]
    fn test_note_from_session() {
        let session = crate::MetaSessionState {
            meta_session_id: "01TEST".to_string(),
            description: Some("test desc".to_string()),
            project_path: "/tmp".to_string(),
            branch: None,
            created_at: chrono::Utc::now(),
            last_accessed: chrono::Utc::now(),
            genealogy: Default::default(),
            tools: {
                let mut m = std::collections::HashMap::new();
                m.insert(
                    "codex".to_string(),
                    crate::state::ToolState {
                        provider_session_id: None,
                        last_action_summary: String::new(),
                        last_exit_code: 0,
                        token_usage: None,
                        updated_at: chrono::Utc::now(),
                    },
                );
                m
            },
            context_status: Default::default(),
            total_token_usage: Some(crate::state::TokenUsage {
                input_tokens: Some(1000),
                output_tokens: Some(500),
                total_tokens: Some(1500),
                estimated_cost_usd: None,
            }),
            phase: crate::state::SessionPhase::Retired,
            task_context: Default::default(),
            turn_count: 5,
            token_budget: None,
            sandbox_info: None,

            termination_reason: None,
            is_seed_candidate: false,
            git_head_at_creation: None,
            last_return_packet: None,
            fork_call_timestamps: Vec::new(),
        };

        let note = note_from_session(&session);
        assert_eq!(note.session_id, "01TEST");
        assert_eq!(note.tool, Some("codex".to_string()));
        assert_eq!(note.status, "Retired");
        assert_eq!(note.turn_count, 5);
        assert!(note.token_usage.is_some());
        let usage = note.token_usage.unwrap();
        assert_eq!(usage.input_tokens, 1000);
        assert_eq!(usage.output_tokens, 500);
        assert_eq!(note.description, Some("test desc".to_string()));
    }

    #[test]
    fn test_list_checkpoints_skips_malformed_note_with_warning() {
        let tmp = tempdir().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        crate::git::ensure_git_init(&sessions_dir).unwrap();

        // Create a commit
        let session_id = ulid::Ulid::new().to_string();
        let session_dir = sessions_dir.join(&session_id);
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(session_dir.join("state.toml"), "test = true").unwrap();
        crate::git::commit_session(&sessions_dir, &session_id, "test").unwrap();

        let head = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&sessions_dir)
            .output()
            .unwrap();
        let head_sha = String::from_utf8_lossy(&head.stdout).trim().to_string();

        // Write malformed note directly via git
        let output = Command::new("git")
            .args([
                "notes",
                "--ref=refs/notes/csa-checkpoints",
                "add",
                "-f",
                "-m",
                "this is not valid TOML {{{",
                &head_sha,
            ])
            .current_dir(&sessions_dir)
            .output()
            .unwrap();
        assert!(output.status.success());

        // list_checkpoints should return empty (malformed note skipped)
        let results = list_checkpoints(&sessions_dir).unwrap();
        assert!(results.is_empty(), "Malformed note should be skipped");
    }

    #[test]
    fn test_write_checkpoint_mismatched_session_id_targets_correct_commit() {
        // Demonstrates the importance of the caller overriding session_id:
        // if note.session_id doesn't match any committed session directory,
        // write_checkpoint correctly fails.
        let tmp = tempdir().unwrap();
        let sessions_dir = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        crate::git::ensure_git_init(&sessions_dir).unwrap();

        // Create and commit session A
        let session_a = ulid::Ulid::new().to_string();
        let dir_a = sessions_dir.join(&session_a);
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::write(dir_a.join("state.toml"), "a = true").unwrap();
        crate::git::commit_session(&sessions_dir, &session_a, "session A").unwrap();

        // Build a note with a fabricated session_id that has no commits
        let mut note = make_note();
        note.session_id = "NONEXISTENT_SESSION_ID_00000".to_string();

        let result = write_checkpoint(&sessions_dir, &note);
        assert!(
            result.is_err(),
            "Should fail when session_id has no matching commits"
        );
    }
}
