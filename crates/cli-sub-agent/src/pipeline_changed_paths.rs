//! Changed-path and changed-crate derivation for hook variable injection.
//!
//! Pure functions that map git status output to workspace-aware crate names,
//! making `{CHANGED_PATHS}` and `{CHANGED_CRATES}` available in hook commands.

use std::collections::BTreeSet;
use std::path::Path;

/// Fingerprint data extracted from a `GitWorkspaceSnapshot` for change detection.
///
/// When status text is identical between pre/post snapshots, fingerprint
/// comparison catches content-level mutations that don't alter porcelain output
/// (e.g., a file modified before `csa run` and modified again during it).
#[derive(Debug, Clone, Default)]
pub(crate) struct SnapshotFingerprints {
    pub(crate) tracked_worktree: Option<u64>,
    pub(crate) tracked_index: Option<u64>,
    pub(crate) untracked: Option<u64>,
}

/// Compute changed file paths by diffing pre-run and post-run git status.
///
/// When fingerprints are provided and status text is identical but fingerprints
/// differ, all paths from the post-status are reported (since fingerprints are
/// aggregate hashes and cannot pinpoint individual files).
///
/// Returns an empty vec when either snapshot is unavailable (e.g., not inside
/// a git worktree, or for events that fire before tool execution).
pub(crate) fn compute_changed_paths(
    pre_status: Option<&str>,
    post_status: Option<&str>,
    pre_fingerprints: Option<&SnapshotFingerprints>,
    post_fingerprints: Option<&SnapshotFingerprints>,
) -> Vec<String> {
    let (Some(pre), Some(post)) = (pre_status, post_status) else {
        return Vec::new();
    };

    // If status is identical, check fingerprints for content-level changes.
    if pre == post {
        if fingerprints_differ(pre_fingerprints, post_fingerprints) {
            // Fingerprints are aggregate — report all paths from post-status.
            return paths_from_porcelain_status(post).into_iter().collect();
        }
        return Vec::new();
    }

    let pre_paths: BTreeSet<String> = paths_from_porcelain_status(pre);
    let post_paths: BTreeSet<String> = paths_from_porcelain_status(post);

    // Changed = (paths in post that weren't in pre) ∪ (paths that existed in
    // both but whose status entry differs).  For simplicity, report all paths
    // present in `post` that are absent from or different in `pre`, plus paths
    // removed from `pre` (deleted during execution).
    let pre_entries: BTreeSet<String> = collect_status_entries(pre);
    let post_entries: BTreeSet<String> = collect_status_entries(post);

    let mut changed = BTreeSet::new();

    // New or modified entries in post.
    for entry in &post_entries {
        if !pre_entries.contains(entry)
            && let Some(path) = path_from_entry(entry)
        {
            changed.insert(path);
        }
    }

    // Entries that disappeared (files cleaned/deleted during execution).
    for entry in &pre_entries {
        if !post_entries.contains(entry)
            && let Some(path) = path_from_entry(entry)
        {
            changed.insert(path);
        }
    }

    // Fallback: if the entry-level diff is empty but status strings differ
    // (e.g., only fingerprint changed), use the symmetric difference of paths.
    if changed.is_empty() {
        for path in pre_paths.symmetric_difference(&post_paths) {
            changed.insert(path.clone());
        }
    }

    changed.into_iter().collect()
}

/// Returns true when fingerprints are available and at least one differs.
fn fingerprints_differ(
    pre: Option<&SnapshotFingerprints>,
    post: Option<&SnapshotFingerprints>,
) -> bool {
    let (Some(pre), Some(post)) = (pre, post) else {
        return false;
    };
    pre.tracked_worktree != post.tracked_worktree
        || pre.tracked_index != post.tracked_index
        || pre.untracked != post.untracked
}

/// Derive workspace crate names from a list of changed file paths.
///
/// For each changed path, check whether it falls under a `crates/<dir>/`
/// prefix.  If a `Cargo.toml` exists at `project_root/crates/<dir>/`, read
/// the `[package] name` field.  Returns deduplicated, sorted crate names.
///
/// This is a pure-ish function (only reads `Cargo.toml` files, no git).
pub(crate) fn derive_changed_crates(project_root: &Path, changed_paths: &[String]) -> Vec<String> {
    let mut crate_names = BTreeSet::new();

    for path in changed_paths {
        if let Some(crate_dir) = extract_crate_dir(path) {
            let cargo_toml = project_root.join(&crate_dir).join("Cargo.toml");
            if let Some(name) = read_crate_name(&cargo_toml) {
                crate_names.insert(name);
            }
        }
    }

    crate_names.into_iter().collect()
}

/// Extract the crate directory prefix from a file path.
///
/// Matches paths like `crates/csa-hooks/src/runner.rs` → `crates/csa-hooks`.
/// Also handles top-level crate paths (files directly under a directory with
/// Cargo.toml, but we only look for the `crates/` prefix pattern).
fn extract_crate_dir(path: &str) -> Option<String> {
    // Normalize path separators.
    let path = path.replace('\\', "/");

    // Look for `crates/<name>/...` pattern.
    if let Some(rest) = path.strip_prefix("crates/") {
        // rest = "csa-hooks/src/runner.rs" or "csa-hooks/Cargo.toml"
        if let Some(slash_pos) = rest.find('/') {
            let dir_name = &rest[..slash_pos];
            if !dir_name.is_empty() {
                return Some(format!("crates/{dir_name}"));
            }
        }
        // File directly in crates/ (unlikely but handle gracefully).
        return None;
    }

    None
}

/// Read the `[package] name` from a Cargo.toml file.
///
/// Uses minimal TOML parsing — just looks for `name = "..."` under `[package]`.
/// This avoids pulling in a full TOML parser dependency for a simple lookup.
fn read_crate_name(cargo_toml_path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(cargo_toml_path).ok()?;
    let mut in_package_section = false;

    for line in content.lines() {
        let trimmed = line.trim();

        // Detect section headers.
        if trimmed.starts_with('[') {
            in_package_section = trimmed == "[package]";
            continue;
        }

        if in_package_section {
            // Match `name = "crate-name"` or `name = 'crate-name'`.
            if let Some(rest) = trimmed.strip_prefix("name") {
                let rest = rest.trim_start();
                if let Some(rest) = rest.strip_prefix('=') {
                    let rest = rest.trim();
                    if let Some(name) = rest.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
                        return Some(name.to_string());
                    }
                    if let Some(name) = rest.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')) {
                        return Some(name.to_string());
                    }
                }
            }
        }
    }

    None
}

/// Format changed paths as a JSON array string for the `{CHANGED_PATHS}` variable.
pub(crate) fn format_changed_paths_json(paths: &[String]) -> String {
    serde_json::to_string(paths).unwrap_or_else(|_| "[]".to_string())
}

/// Format changed crates as a space-separated string for `{CHANGED_CRATES}`.
pub(crate) fn format_changed_crates(crates: &[String]) -> String {
    crates.join(" ")
}

/// Format changed crates as pre-escaped `-p <crate>` flags for cargo commands.
///
/// Produces `-p crate1 -p crate2` which can be injected raw into shell commands
/// via `{!CHANGED_CRATES_FLAGS}` (raw substitution). Each crate name is
/// individually shell-escaped to prevent injection.
pub(crate) fn format_changed_crates_flags(crates: &[String]) -> String {
    crates
        .iter()
        .map(|c| format!("-p '{}'", c.replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join(" ")
}

// -- Internal helpers --

fn paths_from_porcelain_status(status: &str) -> BTreeSet<String> {
    collect_status_entries(status)
        .into_iter()
        .filter_map(|entry| path_from_entry(&entry))
        .collect()
}

fn collect_status_entries(status: &str) -> BTreeSet<String> {
    if status.contains('\0') {
        status
            .split('\0')
            .filter(|e| !e.is_empty())
            .map(|e| e.to_string())
            .collect()
    } else {
        status
            .lines()
            .filter(|e| !e.is_empty())
            .map(|e| e.to_string())
            .collect()
    }
}

fn path_from_entry(entry: &str) -> Option<String> {
    // Porcelain v1 format: XY<space>path
    if entry.len() < 4 {
        return None;
    }
    let third = entry.as_bytes().get(2).copied()?;
    if third != b' ' {
        return None;
    }
    let path = entry.get(3..)?;
    if path.is_empty() {
        return None;
    }
    Some(path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // -- extract_crate_dir tests --

    #[test]
    fn extract_crate_dir_standard_path() {
        assert_eq!(
            extract_crate_dir("crates/csa-hooks/src/runner.rs"),
            Some("crates/csa-hooks".to_string())
        );
    }

    #[test]
    fn extract_crate_dir_cargo_toml() {
        assert_eq!(
            extract_crate_dir("crates/weave/Cargo.toml"),
            Some("crates/weave".to_string())
        );
    }

    #[test]
    fn extract_crate_dir_non_crate_path() {
        assert_eq!(extract_crate_dir("src/main.rs"), None);
        assert_eq!(extract_crate_dir("README.md"), None);
        assert_eq!(extract_crate_dir(".github/workflows/ci.yml"), None);
    }

    #[test]
    fn extract_crate_dir_windows_separator() {
        assert_eq!(
            extract_crate_dir("crates\\csa-hooks\\src\\runner.rs"),
            Some("crates/csa-hooks".to_string())
        );
    }

    #[test]
    fn extract_crate_dir_file_directly_in_crates() {
        // e.g., "crates/README.md" — no crate subdirectory
        assert_eq!(extract_crate_dir("crates/README.md"), None);
    }

    // -- read_crate_name tests --

    #[test]
    fn read_crate_name_standard_cargo_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let cargo_toml = tmp.path().join("Cargo.toml");
        fs::write(
            &cargo_toml,
            r#"[package]
name = "my-crate"
version = "0.1.0"
edition = "2021"
"#,
        )
        .unwrap();

        assert_eq!(read_crate_name(&cargo_toml), Some("my-crate".to_string()));
    }

    #[test]
    fn read_crate_name_workspace_version() {
        let tmp = tempfile::tempdir().unwrap();
        let cargo_toml = tmp.path().join("Cargo.toml");
        fs::write(
            &cargo_toml,
            r#"[package]
name = "csa-hooks"
version.workspace = true
edition.workspace = true
"#,
        )
        .unwrap();

        assert_eq!(read_crate_name(&cargo_toml), Some("csa-hooks".to_string()));
    }

    #[test]
    fn read_crate_name_missing_file() {
        assert_eq!(read_crate_name(Path::new("/nonexistent/Cargo.toml")), None);
    }

    #[test]
    fn read_crate_name_no_package_section() {
        let tmp = tempfile::tempdir().unwrap();
        let cargo_toml = tmp.path().join("Cargo.toml");
        fs::write(
            &cargo_toml,
            r#"[workspace]
members = ["crates/*"]
"#,
        )
        .unwrap();

        assert_eq!(read_crate_name(&cargo_toml), None);
    }

    // -- derive_changed_crates tests --

    #[test]
    fn derive_changed_crates_from_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let project_root = tmp.path();

        // Create two fake crates
        let hooks_dir = project_root.join("crates/csa-hooks");
        fs::create_dir_all(&hooks_dir).unwrap();
        fs::write(
            hooks_dir.join("Cargo.toml"),
            "[package]\nname = \"csa-hooks\"\n",
        )
        .unwrap();

        let session_dir = project_root.join("crates/csa-session");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join("Cargo.toml"),
            "[package]\nname = \"csa-session\"\n",
        )
        .unwrap();

        let paths = vec![
            "crates/csa-hooks/src/runner.rs".to_string(),
            "crates/csa-hooks/src/config.rs".to_string(),
            "crates/csa-session/src/lib.rs".to_string(),
            "README.md".to_string(), // non-crate path
        ];

        let crates = derive_changed_crates(project_root, &paths);
        assert_eq!(crates, vec!["csa-hooks", "csa-session"]);
    }

    #[test]
    fn derive_changed_crates_no_crate_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = vec!["README.md".to_string(), "scripts/test.sh".to_string()];

        let crates = derive_changed_crates(tmp.path(), &paths);
        assert!(crates.is_empty());
    }

    #[test]
    fn derive_changed_crates_missing_cargo_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let project_root = tmp.path();
        // Create directory but no Cargo.toml
        fs::create_dir_all(project_root.join("crates/ghost-crate")).unwrap();

        let paths = vec!["crates/ghost-crate/src/lib.rs".to_string()];
        let crates = derive_changed_crates(project_root, &paths);
        assert!(crates.is_empty());
    }

    // -- compute_changed_paths tests --

    #[test]
    fn compute_changed_paths_no_snapshots() {
        assert!(compute_changed_paths(None, None, None, None).is_empty());
        assert!(compute_changed_paths(Some(""), None, None, None).is_empty());
        assert!(compute_changed_paths(None, Some(""), None, None).is_empty());
    }

    #[test]
    fn compute_changed_paths_identical_status() {
        let status = " M src/lib.rs";
        assert!(compute_changed_paths(Some(status), Some(status), None, None).is_empty());
    }

    #[test]
    fn compute_changed_paths_new_file_in_post() {
        let pre = " M src/lib.rs";
        let post = " M src/lib.rs\n?? src/new.rs";
        let paths = compute_changed_paths(Some(pre), Some(post), None, None);
        assert!(paths.contains(&"src/new.rs".to_string()));
    }

    #[test]
    fn compute_changed_paths_file_deleted_in_post() {
        let pre = " M src/old.rs\n M src/lib.rs";
        let post = " M src/lib.rs";
        let paths = compute_changed_paths(Some(pre), Some(post), None, None);
        assert!(paths.contains(&"src/old.rs".to_string()));
    }

    #[test]
    fn compute_changed_paths_nul_separated() {
        let pre = " M src/lib.rs\0";
        let post = " M src/lib.rs\0?? src/new.rs\0";
        let paths = compute_changed_paths(Some(pre), Some(post), None, None);
        assert!(paths.contains(&"src/new.rs".to_string()));
    }

    #[test]
    fn compute_changed_paths_identical_status_different_fingerprints() {
        let status = " M src/lib.rs";
        let pre_fp = SnapshotFingerprints {
            tracked_worktree: Some(111),
            tracked_index: Some(222),
            untracked: Some(333),
        };
        let post_fp = SnapshotFingerprints {
            tracked_worktree: Some(999),
            tracked_index: Some(222),
            untracked: Some(333),
        };
        let paths =
            compute_changed_paths(Some(status), Some(status), Some(&pre_fp), Some(&post_fp));
        assert!(
            paths.contains(&"src/lib.rs".to_string()),
            "should detect change via fingerprint even when status text is identical"
        );
    }

    #[test]
    fn compute_changed_paths_identical_status_identical_fingerprints() {
        let status = " M src/lib.rs";
        let fp = SnapshotFingerprints {
            tracked_worktree: Some(111),
            tracked_index: Some(222),
            untracked: Some(333),
        };
        let paths = compute_changed_paths(Some(status), Some(status), Some(&fp), Some(&fp));
        assert!(
            paths.is_empty(),
            "no changes when both status and fingerprints are identical"
        );
    }

    // -- format tests --

    #[test]
    fn format_changed_paths_json_produces_valid_json() {
        let paths = vec!["src/foo.rs".to_string(), "src/bar.rs".to_string()];
        let json = format_changed_paths_json(&paths);
        assert_eq!(json, r#"["src/foo.rs","src/bar.rs"]"#);
    }

    #[test]
    fn format_changed_paths_json_empty() {
        let json = format_changed_paths_json(&[]);
        assert_eq!(json, "[]");
    }

    #[test]
    fn format_changed_crates_space_separated() {
        let crates = vec!["csa-hooks".to_string(), "csa-session".to_string()];
        assert_eq!(format_changed_crates(&crates), "csa-hooks csa-session");
    }

    #[test]
    fn format_changed_crates_empty() {
        assert_eq!(format_changed_crates(&[]), "");
    }

    // -- format_changed_crates_flags tests --

    #[test]
    fn format_changed_crates_flags_single() {
        let crates = vec!["csa-hooks".to_string()];
        assert_eq!(format_changed_crates_flags(&crates), "-p 'csa-hooks'");
    }

    #[test]
    fn format_changed_crates_flags_multi() {
        let crates = vec!["csa-hooks".to_string(), "csa-session".to_string()];
        assert_eq!(
            format_changed_crates_flags(&crates),
            "-p 'csa-hooks' -p 'csa-session'"
        );
    }

    #[test]
    fn format_changed_crates_flags_empty() {
        assert_eq!(format_changed_crates_flags(&[]), "");
    }
}
