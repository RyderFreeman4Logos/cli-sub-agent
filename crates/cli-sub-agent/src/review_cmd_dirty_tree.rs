use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use csa_session::{
    Finding, FindingsFile, ReviewFinding, ReviewFindingFileRange, Severity, get_session_dir,
    write_findings_toml,
};
use tracing::{debug, warn};

const REVIEW_WORKTREE_MUTATION_FINDING_ID: &str = "CSA-REVIEW-WORKTREE-MUTATION";
const REVIEW_WORKTREE_MUTATION_RULE_ID: &str = "csa.review.readonly-worktree-mutation";

/// Returns the set of file paths (relative to project root) that have uncommitted
/// changes in the working tree, according to `git diff --name-only HEAD`.
pub(super) fn dirty_files_in_project(project_root: &Path) -> HashSet<String> {
    let Ok(out) = std::process::Command::new("git")
        .args(["diff", "--name-only", "HEAD"])
        .current_dir(project_root)
        .output()
    else {
        return HashSet::new();
    };

    if !out.status.success() {
        return HashSet::new();
    }

    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect()
}

/// Reads `output/findings.toml` from `session_dir` and extracts all referenced file paths,
/// sorted and deduplicated.
pub(super) fn finding_file_paths_from_session(session_dir: &Path) -> Vec<String> {
    let findings_path = session_dir.join("output").join("findings.toml");
    let content = match std::fs::read_to_string(&findings_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    match toml::from_str::<FindingsFile>(&content) {
        Ok(findings_file) => {
            let mut paths: Vec<String> = findings_file
                .findings
                .iter()
                .flat_map(|f| f.file_ranges.iter().map(|r| r.path.clone()))
                .collect();
            paths.sort();
            paths.dedup();
            paths
        }
        Err(_) => Vec::new(),
    }
}

/// Returns finding file paths that appear in `dirty_files`.
pub(super) fn intersect_dirty_finding_files(
    finding_paths: &[String],
    dirty_files: &HashSet<String>,
) -> Vec<String> {
    let mut result: Vec<String> = finding_paths
        .iter()
        .filter(|p| dirty_files.contains(p.as_str()))
        .cloned()
        .collect();
    result.sort();
    result
}

/// Detect finding file paths that have uncommitted changes in the working tree.
///
/// Returns an empty vec when no structured file paths are present in findings.toml
/// or when the working tree is clean.
pub(super) fn detect_dirty_tree_findings(project_root: &Path, session_dir: &Path) -> Vec<String> {
    let finding_paths = finding_file_paths_from_session(session_dir);
    if finding_paths.is_empty() {
        debug!("No structured file paths in findings.toml; skipping dirty-tree check");
        return Vec::new();
    }

    let dirty = dirty_files_in_project(project_root);
    if dirty.is_empty() {
        return Vec::new();
    }

    intersect_dirty_finding_files(&finding_paths, &dirty)
}

/// Resolve the session dir for `session_id` and emit dirty-tree hints for any findings
/// that reference uncommitted files. No-op when the session dir cannot be resolved or
/// when the working tree is clean.
///
/// Informational only — does not affect the review verdict.
pub(super) fn maybe_emit_dirty_tree_hint(project_root: &Path, session_id: Option<&str>) {
    let Some(session_id) = session_id else { return };
    let Ok(session_dir) = get_session_dir(project_root, session_id) else {
        return;
    };
    let dirty = detect_dirty_tree_findings(project_root, &session_dir);
    emit_dirty_tree_hint(&dirty);
}

/// Emit a dirty-tree hint to stderr for each finding that references an uncommitted file.
///
/// Informational only — does not affect the review verdict.
fn emit_dirty_tree_hint(dirty_finding_files: &[String]) {
    for file in dirty_finding_files {
        eprintln!(
            "⚠ Finding references {file} which has uncommitted changes.\n  \
             Review is based on committed HEAD, not the working tree.\n  \
             Consider committing before re-review."
        );
    }
}

/// Convert a read-only review repo-write audit into a blocking structured finding.
///
/// `pipeline_post_exec_audit` records the exact tracked paths that changed during
/// a read-only session. Review consumers gate on `findings.toml` and
/// `review-verdict.json`, so a dirty tracked worktree must be surfaced there
/// rather than remaining a warning-only manager sidecar.
pub(super) fn append_repo_write_audit_finding(
    project_root: &Path,
    session_id: &str,
) -> Vec<Finding> {
    let paths = repo_write_audit_paths(project_root, session_id);
    if paths.is_empty() {
        return Vec::new();
    }

    let review_finding = review_worktree_mutation_finding(&paths);
    let legacy_finding = legacy_worktree_mutation_finding(&paths);
    if let Ok(session_dir) = get_session_dir(project_root, session_id) {
        let findings_path = session_dir.join("output").join("findings.toml");
        let mut findings_file = std::fs::read_to_string(&findings_path)
            .ok()
            .and_then(|content| toml::from_str::<FindingsFile>(&content).ok())
            .unwrap_or_default();
        findings_file
            .findings
            .retain(|finding| finding.id != REVIEW_WORKTREE_MUTATION_FINDING_ID);
        findings_file.findings.push(review_finding);

        if let Err(error) = write_findings_toml(&session_dir, &findings_file) {
            warn!(
                session_id,
                error = %error,
                "Failed to persist review worktree-mutation finding"
            );
        } else {
            let synthetic_marker = session_dir
                .join("output")
                .join(super::findings_toml::FINDINGS_TOML_SYNTHETIC_MARKER);
            let _ = std::fs::remove_file(synthetic_marker);
        }
    }

    eprintln!(
        "[csa-review] Read-only review mutated repo-tracked file(s): {}",
        format_path_list(&paths)
    );
    vec![legacy_finding]
}

/// Return the blocking finding(s) implied by a read-only review repo-write audit
/// without mutating any review sidecars.
pub(super) fn repo_write_audit_findings(project_root: &Path, session_id: &str) -> Vec<Finding> {
    let paths = repo_write_audit_paths(project_root, session_id);
    if paths.is_empty() {
        return Vec::new();
    }
    vec![legacy_worktree_mutation_finding(&paths)]
}

fn repo_write_audit_paths(project_root: &Path, session_id: &str) -> Vec<String> {
    let Some(result) = csa_session::load_result(project_root, session_id)
        .ok()
        .flatten()
    else {
        return Vec::new();
    };
    repo_write_audit_paths_from_result(&result)
}

fn repo_write_audit_paths_from_result(result: &csa_session::SessionResult) -> Vec<String> {
    let Some(audit) = result
        .manager_fields
        .artifacts
        .as_ref()
        .and_then(|value| value.get("repo_write_audit"))
        .and_then(toml::Value::as_table)
    else {
        return Vec::new();
    };

    let mut paths = BTreeSet::new();
    collect_path_array(audit.get("added"), &mut paths);
    collect_path_array(audit.get("modified"), &mut paths);
    collect_path_array(audit.get("deleted"), &mut paths);
    collect_renamed_paths(audit.get("renamed"), &mut paths);
    paths.into_iter().collect()
}

fn collect_path_array(value: Option<&toml::Value>, paths: &mut BTreeSet<String>) {
    let Some(array) = value.and_then(toml::Value::as_array) else {
        return;
    };
    for item in array {
        if let Some(path) = item.as_str().map(str::trim).filter(|path| !path.is_empty()) {
            paths.insert(path.to_string());
        }
    }
}

fn collect_renamed_paths(value: Option<&toml::Value>, paths: &mut BTreeSet<String>) {
    let Some(array) = value.and_then(toml::Value::as_array) else {
        return;
    };
    for item in array {
        let Some(table) = item.as_table() else {
            continue;
        };
        collect_path_value(table.get("from"), paths);
        collect_path_value(table.get("to"), paths);
    }
}

fn collect_path_value(value: Option<&toml::Value>, paths: &mut BTreeSet<String>) {
    if let Some(path) = value
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
    {
        paths.insert(path.to_string());
    }
}

fn review_worktree_mutation_finding(paths: &[String]) -> ReviewFinding {
    ReviewFinding {
        id: REVIEW_WORKTREE_MUTATION_FINDING_ID.to_string(),
        severity: Severity::High,
        file_ranges: paths
            .iter()
            .map(|path| ReviewFindingFileRange {
                path: path.clone(),
                start: 1,
                end: None,
            })
            .collect(),
        is_regression_of_commit: None,
        suggested_test_scenario: Some(
            "Run csa review again after restoring or intentionally committing the tracked worktree changes."
                .to_string(),
        ),
        description: format!(
            "Read-only review mutated repo-tracked file(s): {}. Treat the review as blocking until the worktree is inspected or restored.",
            format_path_list(paths)
        ),
    }
}

fn legacy_worktree_mutation_finding(paths: &[String]) -> Finding {
    Finding {
        severity: Severity::High,
        fid: REVIEW_WORKTREE_MUTATION_FINDING_ID.to_string(),
        file: paths
            .first()
            .cloned()
            .unwrap_or_else(|| "unknown".to_string()),
        line: Some(1),
        rule_id: REVIEW_WORKTREE_MUTATION_RULE_ID.to_string(),
        summary: format!(
            "Read-only review mutated repo-tracked file(s): {}",
            format_path_list(paths)
        ),
        engine: "csa-review".to_string(),
    }
}

fn format_path_list(paths: &[String]) -> String {
    const MAX_SHOWN: usize = 8;
    let shown = paths
        .iter()
        .take(MAX_SHOWN)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if paths.len() > MAX_SHOWN {
        format!("{shown}, and {} more", paths.len() - MAX_SHOWN)
    } else {
        shown
    }
}

#[cfg(test)]
#[path = "review_cmd_dirty_tree_tests.rs"]
mod sidecar_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersect_dirty_finding_files_returns_overlap() {
        let finding_paths = vec!["src/main.rs".to_string(), "src/lib.rs".to_string()];
        let dirty: HashSet<String> = ["src/main.rs", "Cargo.toml"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let result = intersect_dirty_finding_files(&finding_paths, &dirty);
        assert_eq!(result, vec!["src/main.rs"]);
    }

    #[test]
    fn intersect_dirty_finding_files_returns_empty_when_no_overlap() {
        let finding_paths = vec!["src/foo.rs".to_string()];
        let dirty: HashSet<String> = ["src/bar.rs"].iter().map(|s| s.to_string()).collect();
        let result = intersect_dirty_finding_files(&finding_paths, &dirty);
        assert!(result.is_empty());
    }

    #[test]
    fn intersect_dirty_finding_files_empty_inputs() {
        let empty_dirty: HashSet<String> = HashSet::new();
        assert!(intersect_dirty_finding_files(&[], &empty_dirty).is_empty());
        assert!(
            intersect_dirty_finding_files(&[], &["a.rs".to_string()].into_iter().collect())
                .is_empty()
        );
        assert!(intersect_dirty_finding_files(&["a.rs".to_string()], &HashSet::new()).is_empty());
    }

    #[test]
    fn finding_file_paths_extracts_sorted_deduped_paths() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let output_dir = tmp.path().join("output");
        std::fs::create_dir_all(&output_dir).expect("create output dir");

        let findings_toml = r#"
[[findings]]
id = "R01"
severity = "high"
description = "test finding"

[[findings.file_ranges]]
path = "crates/foo/src/lib.rs"
start = 10

[[findings.file_ranges]]
path = "crates/bar/src/main.rs"
start = 5

[[findings]]
id = "R02"
severity = "medium"
description = "another finding"

[[findings.file_ranges]]
path = "crates/foo/src/lib.rs"
start = 42
end = 50
"#;
        std::fs::write(output_dir.join("findings.toml"), findings_toml)
            .expect("write findings.toml");

        let paths = finding_file_paths_from_session(tmp.path());
        assert_eq!(
            paths,
            vec!["crates/bar/src/main.rs", "crates/foo/src/lib.rs"]
        );
    }

    #[test]
    fn finding_file_paths_returns_empty_when_no_findings_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = finding_file_paths_from_session(tmp.path());
        assert!(paths.is_empty());
    }

    #[test]
    fn finding_file_paths_returns_empty_when_no_file_ranges() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let output_dir = tmp.path().join("output");
        std::fs::create_dir_all(&output_dir).expect("create output dir");

        let findings_toml = r#"
[[findings]]
id = "R01"
severity = "high"
description = "finding with no file ranges"
"#;
        std::fs::write(output_dir.join("findings.toml"), findings_toml)
            .expect("write findings.toml");

        let paths = finding_file_paths_from_session(tmp.path());
        assert!(paths.is_empty());
    }

    #[test]
    fn detect_dirty_tree_findings_returns_empty_when_findings_toml_missing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // No findings.toml written — session_dir has no output/findings.toml
        let result = detect_dirty_tree_findings(tmp.path(), tmp.path());
        assert!(result.is_empty());
    }
}
