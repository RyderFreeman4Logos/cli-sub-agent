use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::warn;

const REVIEW_RELATIVE_ARTIFACT_GUARD_ENV: &str = "CSA_REVIEW_ALLOW_RELATIVE_ARTIFACTS";
const REPO_ROOT_GUARDED_REVIEW_ARTIFACTS: &[&str] = &[
    "review-findings.json",
    "review-report.md",
    "review-verdict.json",
    "result.toml",
    "summary.md",
    "details.md",
];

pub(super) fn detect_repo_root_review_artifact_violations(
    project_root: &Path,
    execution_started_at: DateTime<Utc>,
) -> Result<Option<Vec<String>>> {
    if std::env::var_os(REVIEW_RELATIVE_ARTIFACT_GUARD_ENV).as_deref() == Some("1".as_ref()) {
        warn!(
            "{}=1 bypasses the review artifact contract guard",
            REVIEW_RELATIVE_ARTIFACT_GUARD_ENV
        );
        return Ok(None);
    }

    let started_at = SystemTime::from(execution_started_at)
        .checked_sub(Duration::from_secs(1))
        .unwrap_or(UNIX_EPOCH);
    let mut leaked_paths = Vec::new();
    collect_guarded_review_artifacts(
        &project_root.join("output"),
        "output",
        started_at,
        &mut leaked_paths,
        is_guarded_repo_root_review_artifact,
    )?;
    collect_guarded_review_artifacts(
        project_root,
        "",
        started_at,
        &mut leaked_paths,
        is_guarded_repo_root_direct_review_artifact,
    )?;

    if leaked_paths.is_empty() {
        Ok(None)
    } else {
        leaked_paths.sort();
        Ok(Some(leaked_paths))
    }
}

fn is_guarded_repo_root_review_artifact(file_name: &str) -> bool {
    matches!(file_name, "result.toml" | "summary.md" | "details.md")
        || file_name.starts_with("review-")
}

fn is_guarded_repo_root_direct_review_artifact(file_name: &str) -> bool {
    REPO_ROOT_GUARDED_REVIEW_ARTIFACTS.contains(&file_name)
}

fn collect_guarded_review_artifacts(
    dir: &Path,
    prefix: &str,
    started_at: SystemTime,
    leaked_paths: &mut Vec<String>,
    is_guarded: fn(&str) -> bool,
) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }

    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }

        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        if !is_guarded(file_name) {
            continue;
        }

        let modified_at = entry.metadata()?.modified().with_context(|| {
            format!(
                "failed to read modified time for {}",
                entry.path().display()
            )
        })?;
        if modified_at >= started_at {
            let relative_path = if prefix.is_empty() {
                file_name.to_string()
            } else {
                format!("{prefix}/{file_name}")
            };
            leaked_paths.push(relative_path);
        }
    }

    Ok(())
}
