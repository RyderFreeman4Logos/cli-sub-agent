//! SHA-pinned review gate markers for fast pre-push verification.
//!
//! After a passing `csa review`, a marker file is written to
//! `.csa/state/review-gate/<branch_safe>-<short_sha>.pass` containing review
//! metadata.  The pre-push hook stat-checks this file for a millisecond fast
//! path, avoiding the expensive session-scan normally required by
//! `csa review --check-verdict`.
//!
//! New commits automatically invalidate old markers because the SHA changes.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::Deserialize;
use tracing::{debug, warn};

/// Number of characters from HEAD SHA used in the marker filename.
const SHORT_SHA_LEN: usize = 11;

/// Default marker retention when GC does not specify a max age.
pub(crate) const DEFAULT_RETENTION_DAYS: i64 = 7;

/// Gate marker directory relative to project root.
const GATE_DIR: &str = ".csa/state/review-gate";

/// Statistics returned by [`gc_review_gate_markers`].
pub(crate) struct GcReviewGateStats {
    pub markers_removed: u64,
}

/// Parsed review-gate marker content.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ReviewGateMarker {
    pub session_id: String,
    pub timestamp: String,
    pub branch: String,
    pub head_sha: String,
    pub scope: String,
    #[serde(default = "default_marker_verdict")]
    pub verdict: String,
    /// Review mode that produced this marker ("standard" or "red-team").
    /// Absent for legacy markers written before review-mode auditing (#1817).
    #[serde(default)]
    pub review_mode: Option<String>,
}

fn default_marker_verdict() -> String {
    "CLEAN".to_string()
}

/// Return the review-gate directory for the given project root.
fn gate_dir(project_root: &Path) -> PathBuf {
    project_root.join(GATE_DIR)
}

/// Sanitize a branch name so it is safe as a filename component.
///
/// `/` → `__`; any other character outside `[a-zA-Z0-9._-]` → `_`.
fn sanitize_branch(branch: &str) -> String {
    branch
        .chars()
        .flat_map(|c| {
            if c == '/' {
                vec!['_', '_']
            } else if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                vec![c]
            } else {
                vec!['_']
            }
        })
        .collect()
}

/// Build the full path of a review-gate marker file.
pub(crate) fn marker_path(project_root: &Path, branch: &str, head_sha: &str) -> PathBuf {
    let short_sha = &head_sha[..head_sha.len().min(SHORT_SHA_LEN)];
    let safe_branch = sanitize_branch(branch);
    gate_dir(project_root).join(format!("{safe_branch}-{short_sha}.pass"))
}

/// TOML content written to the marker file.
///
/// `review_mode` is emitted only when known so legacy markers (and standard
/// reviews where the caller did not resolve a mode) stay deserializable via the
/// `#[serde(default)]` on [`ReviewGateMarker::review_mode`].
fn marker_toml(
    session_id: &str,
    branch: &str,
    head_sha: &str,
    scope: &str,
    review_mode: Option<&str>,
) -> String {
    let ts = Utc::now().to_rfc3339();
    let review_mode_line = match review_mode {
        Some(mode) => format!("review_mode = {mode:?}\n"),
        None => String::new(),
    };
    format!(
        "session_id = {session_id:?}\ntimestamp = {ts:?}\nbranch = {branch:?}\nhead_sha = {head_sha:?}\nscope = {scope:?}\nverdict = \"CLEAN\"\n{review_mode_line}"
    )
}

/// Read the deterministic marker for the current branch and commit.
///
/// Best-effort: corrupt or unreadable markers are treated as absent so callers
/// can fall back to the slower session scan.
pub(crate) fn read_review_gate_marker(
    project_root: &Path,
    branch: &str,
    head_sha: &str,
) -> Option<ReviewGateMarker> {
    let path = marker_path(project_root, branch, head_sha);
    if !path.exists() {
        return None;
    }
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) => {
            warn!(
                path = %path.display(),
                error = %e,
                "Failed to read review-gate marker; falling back to session scan"
            );
            return None;
        }
    };
    match toml::from_str(&raw) {
        Ok(marker) => Some(marker),
        Err(e) => {
            warn!(
                path = %path.display(),
                error = %e,
                "Failed to parse review-gate marker; falling back to session scan"
            );
            None
        }
    }
}

/// Write a review-gate marker for a passing review.
///
/// Best-effort: failures are logged as warnings and do not abort the review.
pub(crate) fn write_review_gate_marker(
    project_root: &Path,
    branch: &str,
    head_sha: &str,
    session_id: &str,
    scope: &str,
    review_mode: Option<&str>,
) {
    if branch.is_empty() || head_sha.is_empty() {
        debug!("Skipping review-gate marker: branch or head_sha is empty");
        return;
    }
    let dir = gate_dir(project_root);
    if let Err(e) = fs::create_dir_all(&dir) {
        warn!(
            dir = %dir.display(),
            error = %e,
            "Failed to create review-gate directory; skipping marker write"
        );
        return;
    }
    let path = marker_path(project_root, branch, head_sha);
    let content = marker_toml(session_id, branch, head_sha, scope, review_mode);
    if let Err(e) = fs::write(&path, &content) {
        warn!(
            path = %path.display(),
            error = %e,
            "Failed to write review-gate marker"
        );
    } else {
        debug!(
            path = %path.display(),
            session_id,
            branch,
            head_sha = &head_sha[..head_sha.len().min(SHORT_SHA_LEN)],
            "Wrote review-gate marker"
        );
    }
}

/// Write a gate marker when `verdict == "CLEAN"`.  No-op for any other verdict.
pub(crate) fn maybe_write_gate_marker_for_clean(
    project_root: &Path,
    head_sha: &str,
    verdict: &str,
    first_session_id: Option<&str>,
    scope: &str,
    review_mode: Option<&str>,
) {
    if verdict != "CLEAN" {
        return;
    }
    if let Some(sid) = first_session_id {
        maybe_write_review_gate_marker(project_root, head_sha, sid, scope, review_mode);
    }
}

/// Resolve the current VCS branch and HEAD SHA, then call [`write_review_gate_marker`].
///
/// Best-effort: if branch or SHA cannot be determined the call is a no-op.
pub(crate) fn maybe_write_review_gate_marker(
    project_root: &Path,
    head_sha: &str,
    session_id: &str,
    scope: &str,
    review_mode: Option<&str>,
) {
    let backend = csa_session::create_vcs_backend(project_root);
    let branch = match backend.identity(project_root) {
        Ok(identity) => identity.ref_name.unwrap_or_default(),
        Err(e) => {
            debug!(error = %e, "Could not resolve VCS identity for review-gate marker");
            String::new()
        }
    };
    if branch.is_empty() {
        debug!("Skipping review-gate marker: no branch resolved");
        return;
    }
    write_review_gate_marker(
        project_root,
        &branch,
        head_sha,
        session_id,
        scope,
        review_mode,
    );
}

/// Remove stale review-gate markers.
///
/// A marker is stale when:
/// - Its modification time exceeds `retention_days`, OR
/// - Its branch no longer exists in the local git repo.
///
/// Returns stats about what was (or would be) removed.
pub(crate) fn gc_review_gate_markers(
    project_root: &Path,
    dry_run: bool,
    retention_days: i64,
) -> GcReviewGateStats {
    let mut markers_removed: u64 = 0;
    let dir = gate_dir(project_root);
    if !dir.exists() {
        return GcReviewGateStats { markers_removed };
    }

    let existing_branches = enumerate_local_branches(project_root);
    let now = Utc::now();

    let Ok(entries) = fs::read_dir(&dir) else {
        return GcReviewGateStats { markers_removed };
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "pass") {
            continue;
        }
        let stale = is_marker_stale(&path, &existing_branches, &now, retention_days);
        if !stale {
            continue;
        }
        if dry_run {
            eprintln!(
                "[dry-run] Would remove stale review-gate marker: {}",
                path.display()
            );
            markers_removed += 1;
        } else if fs::remove_file(&path).is_ok() {
            debug!(path = %path.display(), "Removed stale review-gate marker");
            markers_removed += 1;
        }
    }

    GcReviewGateStats { markers_removed }
}

fn is_marker_stale(
    path: &Path,
    existing_branches: &[String],
    now: &chrono::DateTime<Utc>,
    retention_days: i64,
) -> bool {
    // Age-based staleness.
    if let Ok(meta) = fs::metadata(path)
        && let Ok(modified) = meta.modified()
        && let Ok(duration) = now
            .signed_duration_since(chrono::DateTime::<Utc>::from(modified))
            .to_std()
        && duration.as_secs() > (retention_days as u64) * 86_400
    {
        return true;
    }

    // Branch-based staleness: parse branch from marker filename stem.
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        // Stem format: <branch_safe>-<short_sha>
        // We cannot unsanitize `__` → `/` reliably, so we re-sanitize each
        // known branch and compare.
        let branch_gone = !existing_branches
            .iter()
            .any(|b| stem.starts_with(&sanitize_branch(b)));
        if branch_gone {
            return true;
        }
    }

    false
}

/// List local git branch names by running `git branch --format=%(refname:short)`.
fn enumerate_local_branches(project_root: &Path) -> Vec<String> {
    let output = std::process::Command::new("git")
        .args(["branch", "--format=%(refname:short)"])
        .current_dir(project_root)
        .output();
    match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn init_git_repo(dir: &Path) {
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir)
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .status()
            .unwrap();
    }

    #[test]
    fn sanitize_branch_replaces_slash() {
        assert_eq!(sanitize_branch("feat/my-feature"), "feat__my-feature");
    }

    #[test]
    fn sanitize_branch_replaces_spaces() {
        assert_eq!(sanitize_branch("fix bad chars!"), "fix_bad_chars_");
    }

    #[test]
    fn marker_path_uses_short_sha_and_safe_branch() {
        let dir = PathBuf::from("/tmp/proj");
        let path = marker_path(&dir, "feat/1337-thing", "abcdef12345678");
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(filename, "feat__1337-thing-abcdef12345.pass");
    }

    #[test]
    fn write_and_stat_marker() {
        let td = TempDir::new().unwrap();
        let project_root = td.path();
        write_review_gate_marker(
            project_root,
            "feat/test",
            "abc1234567890",
            "SID001",
            "range:main...HEAD",
            Some("red-team"),
        );
        let path = marker_path(project_root, "feat/test", "abc1234567890");
        assert!(
            path.exists(),
            "marker file should exist: {}",
            path.display()
        );
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("SID001"));
        assert!(content.contains("range:main...HEAD"));
        assert!(content.contains("verdict = \"CLEAN\""));
        assert!(content.contains("review_mode = \"red-team\""));
        let marker = read_review_gate_marker(project_root, "feat/test", "abc1234567890")
            .expect("marker should parse");
        assert_eq!(marker.session_id, "SID001");
        assert_eq!(marker.branch, "feat/test");
        assert_eq!(marker.head_sha, "abc1234567890");
        assert_eq!(marker.scope, "range:main...HEAD");
        assert_eq!(marker.verdict, "CLEAN");
        assert_eq!(marker.review_mode.as_deref(), Some("red-team"));
        assert!(!marker.timestamp.is_empty());
    }

    #[test]
    fn marker_without_review_mode_deserializes_to_none() {
        let td = TempDir::new().unwrap();
        let project_root = td.path();
        // A standard review (or legacy marker) omits review_mode entirely.
        write_review_gate_marker(
            project_root,
            "feat/legacy",
            "abc1234567890",
            "SID_LEGACY",
            "range:main...HEAD",
            None,
        );
        let path = marker_path(project_root, "feat/legacy", "abc1234567890");
        let content = fs::read_to_string(&path).unwrap();
        assert!(
            !content.contains("review_mode"),
            "review_mode line must be omitted when unknown"
        );
        let marker = read_review_gate_marker(project_root, "feat/legacy", "abc1234567890")
            .expect("marker should parse");
        assert_eq!(marker.review_mode, None);
    }

    #[test]
    fn gc_removes_old_markers() {
        let td = TempDir::new().unwrap();
        let project_root = td.path();
        init_git_repo(project_root);

        // Write a marker for a branch that doesn't exist in git.
        write_review_gate_marker(
            project_root,
            "old-gone-branch",
            "deadbeef00000",
            "SID_OLD",
            "range:main...HEAD",
            None,
        );
        let marker = marker_path(project_root, "old-gone-branch", "deadbeef00000");
        assert!(marker.exists());

        let stats = gc_review_gate_markers(project_root, false, DEFAULT_RETENTION_DAYS);
        assert_eq!(
            stats.markers_removed, 1,
            "should remove marker for deleted branch"
        );
        assert!(!marker.exists(), "marker should be gone after gc");
    }

    #[test]
    fn gc_preserves_live_branch_markers() {
        let td = TempDir::new().unwrap();
        let project_root = td.path();
        init_git_repo(project_root);

        // The HEAD branch after `git init` is "master" or "main" depending on git config.
        let branch_output = std::process::Command::new("git")
            .args(["branch", "--format=%(refname:short)"])
            .current_dir(project_root)
            .output()
            .unwrap();
        let branch = String::from_utf8_lossy(&branch_output.stdout)
            .lines()
            .next()
            .unwrap_or("master")
            .trim()
            .to_owned();

        write_review_gate_marker(
            project_root,
            &branch,
            "abc0000000011",
            "SID_LIVE",
            "range:main...HEAD",
            None,
        );
        let marker = marker_path(project_root, &branch, "abc0000000011");
        assert!(marker.exists());

        let stats = gc_review_gate_markers(project_root, false, DEFAULT_RETENTION_DAYS);
        // Should NOT remove a marker whose branch still exists (and is recent).
        assert_eq!(
            stats.markers_removed, 0,
            "live branch marker should be kept"
        );
        assert!(marker.exists());
    }

    #[test]
    fn gc_dry_run_does_not_delete() {
        let td = TempDir::new().unwrap();
        let project_root = td.path();
        init_git_repo(project_root);

        write_review_gate_marker(
            project_root,
            "phantom-branch",
            "ffffffff00011",
            "SID_DRY",
            "range:main...HEAD",
            None,
        );
        let marker = marker_path(project_root, "phantom-branch", "ffffffff00011");
        assert!(marker.exists());

        let stats = gc_review_gate_markers(project_root, true, DEFAULT_RETENTION_DAYS);
        assert_eq!(
            stats.markers_removed, 1,
            "dry-run should count but not delete"
        );
        assert!(marker.exists(), "dry-run must not delete the marker");
    }
}
