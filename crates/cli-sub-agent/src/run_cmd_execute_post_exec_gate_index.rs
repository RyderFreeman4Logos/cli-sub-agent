use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub(super) struct TempGateIndex {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

type GateEnv = Option<HashMap<String, String>>;
type GateEnvWithIndex = (GateEnv, Option<TempGateIndex>);

fn git_index_path(project_root: &Path) -> Result<PathBuf> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", "--git-path", "index"])
        .output()
        .with_context(|| format!("failed to locate git index in {}", project_root.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "failed to locate git index in {}: {}",
            project_root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        anyhow::bail!(
            "git returned an empty index path in {}",
            project_root.display()
        );
    }
    let path = PathBuf::from(path);
    Ok(if path.is_absolute() {
        path
    } else {
        project_root.join(path)
    })
}

fn build_temp_gate_index(project_root: &Path, changed_paths: &[String]) -> Result<TempGateIndex> {
    let dir = tempfile::Builder::new()
        .prefix("csa-post-exec-index-")
        .tempdir()
        .context("failed to create temporary post-exec gate index directory")?;
    let path = dir.path().join("index");
    let real_index = git_index_path(project_root)?;
    if real_index.exists() {
        fs::copy(&real_index, &path).with_context(|| {
            format!(
                "failed to copy git index {} for post-exec gate",
                real_index.display()
            )
        })?;
    }

    for changed_path in changed_paths {
        if should_stage_changed_path(project_root, &path, changed_path)? {
            stage_changed_path(project_root, &path, changed_path)?;
        }
    }

    Ok(TempGateIndex { _dir: dir, path })
}

fn should_stage_changed_path(
    project_root: &Path,
    temp_index: &Path,
    changed_path: &str,
) -> Result<bool> {
    if project_root.join(changed_path).symlink_metadata().is_ok() {
        return Ok(true);
    }
    temp_index_contains_path(project_root, temp_index, changed_path)
}

fn temp_index_contains_path(
    project_root: &Path,
    temp_index: &Path,
    changed_path: &str,
) -> Result<bool> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["ls-files", "--stage", "--"])
        .arg(changed_path)
        .env("GIT_INDEX_FILE", temp_index)
        .env("GIT_LITERAL_PATHSPECS", "1")
        .output()
        .with_context(|| {
            format!(
                "failed to inspect temporary index for {}",
                project_root.display()
            )
        })?;
    if !output.status.success() {
        anyhow::bail!(
            "failed to inspect temporary index for changed path {changed_path}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(!output.stdout.is_empty())
}

fn stage_changed_path(project_root: &Path, temp_index: &Path, changed_path: &str) -> Result<()> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["add", "--all", "--"])
        .arg(changed_path)
        .env("GIT_INDEX_FILE", temp_index)
        .env("GIT_LITERAL_PATHSPECS", "1")
        .output()
        .with_context(|| {
            format!(
                "failed to stage session changed path in temporary index for {}",
                project_root.display()
            )
        })?;
    if !output.status.success() {
        anyhow::bail!(
            "failed to stage session changed path {changed_path} in temporary index: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

pub(super) fn post_exec_gate_env_with_temp_index(
    project_root: &Path,
    changed_paths: Option<&[String]>,
    extra_env: GateEnv,
) -> Result<GateEnvWithIndex> {
    let Some(changed_paths) = changed_paths.filter(|paths| !paths.is_empty()) else {
        return Ok((extra_env, None));
    };
    if !crate::run_cmd::is_git_worktree(project_root) {
        return Ok((extra_env, None));
    }

    let temp_index = build_temp_gate_index(project_root, changed_paths)?;
    let mut env = extra_env.unwrap_or_default();
    env.insert(
        "GIT_INDEX_FILE".to_string(),
        temp_index.path.to_string_lossy().into_owned(),
    );
    Ok((Some(env), Some(temp_index)))
}
