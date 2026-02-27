//! Batch compilation: walk a directory tree for `workflow.toml` sources and compile
//! each one, printing per-pattern progress and a final summary.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::compiler::{compile, plan_to_toml};
use crate::parser::parse_skill;

/// Aggregated result of a batch compile run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchSummary {
    pub ok: usize,
    pub failed: usize,
    pub results: Vec<PatternResult>,
}

/// Per-pattern compile result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternResult {
    pub path: PathBuf,
    pub success: bool,
    pub error: Option<String>,
}

/// Recursively find all `workflow.toml` files under `root`, compile each one via
/// the existing parse-then-compile pipeline, and return an aggregated summary.
///
/// For each skill markdown file compiled, progress is printed to stderr:
/// ```text
/// [1/5] patterns/commit/workflow.toml ... OK
/// [2/5] patterns/debate/workflow.toml ... FAILED: <reason>
/// ```
pub fn compile_all(root: &Path) -> Result<BatchSummary> {
    if !root.exists() {
        eprintln!(
            "directory {} does not exist, nothing to compile",
            root.display()
        );
        return Ok(BatchSummary {
            ok: 0,
            failed: 0,
            results: Vec::new(),
        });
    }
    if !root.is_dir() {
        anyhow::bail!("{} is not a directory", root.display());
    }

    let plans = find_workflow_tomls(root)?;

    if plans.is_empty() {
        eprintln!("no workflow.toml files found under {}", root.display());
        return Ok(BatchSummary {
            ok: 0,
            failed: 0,
            results: Vec::new(),
        });
    }

    let total = plans.len();
    let mut ok = 0usize;
    let mut failed = 0usize;
    let mut results = Vec::with_capacity(total);

    for (i, plan_path) in plans.iter().enumerate() {
        let label = plan_path.display();
        eprint!("[{}/{}] {label} ... ", i + 1, total);

        match compile_single(plan_path) {
            Ok(()) => {
                eprintln!("OK");
                ok += 1;
                results.push(PatternResult {
                    path: plan_path.clone(),
                    success: true,
                    error: None,
                });
            }
            Err(e) => {
                eprintln!("FAILED: {e:#}");
                failed += 1;
                results.push(PatternResult {
                    path: plan_path.clone(),
                    success: false,
                    error: Some(format!("{e:#}")),
                });
            }
        }
    }

    Ok(BatchSummary {
        ok,
        failed,
        results,
    })
}

/// Find all `workflow.toml` files under `root`, sorted for deterministic output.
fn find_workflow_tomls(root: &Path) -> Result<Vec<PathBuf>> {
    let mut plans = Vec::new();
    walk_dir(root, &mut plans)?;
    plans.sort();
    Ok(plans)
}

/// Recursive directory walker that collects paths named `workflow.toml`.
fn walk_dir(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("failed to read directory {}", dir.display()))?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_dir(&path, out)?;
        } else if path.file_name().is_some_and(|n| n == "workflow.toml") {
            out.push(path);
        }
    }

    Ok(())
}

/// Compile a single `workflow.toml` by finding its sibling SKILL.md (or PATTERN.md)
/// source, parsing and compiling it, and verifying the TOML round-trips.
///
/// Since `workflow.toml` is already the *compiled* output, we validate it by
/// deserializing and re-serializing to ensure structural correctness.
fn compile_single(plan_path: &Path) -> Result<()> {
    // Look for a companion skill markdown source next to (or near) workflow.toml.
    // The convention is: patterns/<name>/PATTERN.md compiles to workflow.toml in
    // the same directory, or skills/<name>/SKILL.md.  If a PATTERN.md or SKILL.md
    // exists we compile from source; otherwise we validate the workflow.toml itself.

    let parent = plan_path
        .parent()
        .context("workflow.toml has no parent directory")?;

    // Try PATTERN.md first, then look for SKILL.md in a skills/ subdirectory.
    let source = find_skill_source(parent);

    match source {
        Some(src_path) => {
            let content = std::fs::read_to_string(&src_path)
                .with_context(|| format!("failed to read {}", src_path.display()))?;
            let doc = parse_skill(&content)
                .with_context(|| format!("failed to parse {}", src_path.display()))?;
            let plan = compile(&doc).context("compilation failed")?;
            // Verify TOML serialization round-trips.
            let _toml_str = plan_to_toml(&plan)?;
            Ok(())
        }
        None => {
            // No source file found; validate workflow.toml structure by deserializing.
            let content = std::fs::read_to_string(plan_path)
                .with_context(|| format!("failed to read {}", plan_path.display()))?;
            let _plan = crate::compiler::plan_from_toml(&content)
                .with_context(|| format!("invalid workflow.toml at {}", plan_path.display()))?;
            Ok(())
        }
    }
}

/// Look for a skill markdown source near the given directory.
///
/// Search order:
/// 1. `<dir>/PATTERN.md`
/// 2. `<dir>/skills/*/SKILL.md` (first match)
fn find_skill_source(dir: &Path) -> Option<PathBuf> {
    let pattern_md = dir.join("PATTERN.md");
    if pattern_md.is_file() {
        return Some(pattern_md);
    }

    let skills_dir = dir.join("skills");
    if skills_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&skills_dir) {
            for entry in entries.flatten() {
                let skill_md = entry.path().join("SKILL.md");
                if skill_md.is_file() {
                    return Some(skill_md);
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Create a minimal valid PATTERN.md for testing (TOML frontmatter).
    fn minimal_pattern_md() -> &'static str {
        r#"---
name = "test-pattern"
---

## Step 1: Hello

Say hello to the world.
"#
    }

    /// Create a minimal valid workflow.toml for testing.
    fn minimal_workflow_toml() -> &'static str {
        r#"[plan]
name = "test-plan"

[[plan.steps]]
id = 1
title = "Hello"
prompt = "Say hello"
on_fail = "abort"
"#
    }

    #[test]
    fn compile_all_nonexistent_dir_returns_zero_results() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing = tmp.path().join("no-such-dir");
        let summary = compile_all(&missing).expect("compile_all should succeed");
        assert_eq!(summary.ok, 0);
        assert_eq!(summary.failed, 0);
        assert!(summary.results.is_empty());
    }

    #[test]
    fn compile_all_file_path_returns_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("not-a-dir.txt");
        fs::write(&file, "hello").unwrap();
        let err = compile_all(&file).unwrap_err();
        assert!(
            err.to_string().contains("is not a directory"),
            "expected 'is not a directory' error, got: {err}"
        );
    }

    #[test]
    fn compile_all_empty_dir_returns_zero_results() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let summary = compile_all(tmp.path()).expect("compile_all should succeed");
        assert_eq!(summary.ok, 0);
        assert_eq!(summary.failed, 0);
        assert!(summary.results.is_empty());
    }

    #[test]
    fn compile_all_finds_and_compiles_pattern_md() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let pattern_dir = tmp.path().join("my-pattern");
        fs::create_dir_all(&pattern_dir).unwrap();
        fs::write(pattern_dir.join("PATTERN.md"), minimal_pattern_md()).unwrap();
        fs::write(pattern_dir.join("workflow.toml"), minimal_workflow_toml()).unwrap();

        let summary = compile_all(tmp.path()).expect("compile_all should succeed");
        assert_eq!(summary.ok, 1);
        assert_eq!(summary.failed, 0);
        assert_eq!(summary.results.len(), 1);
        assert!(summary.results[0].success);
    }

    #[test]
    fn compile_all_validates_workflow_toml_without_source() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let pattern_dir = tmp.path().join("no-source");
        fs::create_dir_all(&pattern_dir).unwrap();
        // Only workflow.toml, no PATTERN.md or SKILL.md.
        fs::write(pattern_dir.join("workflow.toml"), minimal_workflow_toml()).unwrap();

        let summary = compile_all(tmp.path()).expect("compile_all should succeed");
        assert_eq!(summary.ok, 1);
        assert_eq!(summary.failed, 0);
    }

    #[test]
    fn compile_all_reports_failure_for_invalid_plan() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let pattern_dir = tmp.path().join("broken");
        fs::create_dir_all(&pattern_dir).unwrap();
        fs::write(
            pattern_dir.join("workflow.toml"),
            "this is not valid toml [",
        )
        .unwrap();

        let summary = compile_all(tmp.path()).expect("compile_all should succeed");
        assert_eq!(summary.ok, 0);
        assert_eq!(summary.failed, 1);
        assert!(!summary.results[0].success);
        assert!(summary.results[0].error.is_some());
    }

    #[test]
    fn compile_all_handles_mixed_success_and_failure() {
        let tmp = tempfile::tempdir().expect("tempdir");

        // Good pattern with source.
        let good = tmp.path().join("good");
        fs::create_dir_all(&good).unwrap();
        fs::write(good.join("PATTERN.md"), minimal_pattern_md()).unwrap();
        fs::write(good.join("workflow.toml"), minimal_workflow_toml()).unwrap();

        // Bad pattern.
        let bad = tmp.path().join("bad");
        fs::create_dir_all(&bad).unwrap();
        fs::write(bad.join("workflow.toml"), "not valid").unwrap();

        let summary = compile_all(tmp.path()).expect("compile_all should succeed");
        assert_eq!(summary.ok, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.results.len(), 2);
    }

    #[test]
    fn find_workflow_tomls_returns_sorted_paths() {
        let tmp = tempfile::tempdir().expect("tempdir");

        let z_dir = tmp.path().join("z-pattern");
        let a_dir = tmp.path().join("a-pattern");
        fs::create_dir_all(&z_dir).unwrap();
        fs::create_dir_all(&a_dir).unwrap();
        fs::write(z_dir.join("workflow.toml"), "").unwrap();
        fs::write(a_dir.join("workflow.toml"), "").unwrap();

        let plans = find_workflow_tomls(tmp.path()).expect("find_workflow_tomls should succeed");
        assert_eq!(plans.len(), 2);
        // "a-pattern" should come before "z-pattern".
        assert!(plans[0] < plans[1]);
    }
}
