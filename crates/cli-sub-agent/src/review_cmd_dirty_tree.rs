use std::collections::HashSet;
use std::path::Path;

use csa_session::{FindingsFile, get_session_dir};
use tracing::debug;

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
