use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

pub fn audit_repo_tracked_writes(
    project_root: &Path,
    session_start_time: SystemTime,
) -> Result<Vec<PathBuf>> {
    let output = Command::new("git")
        .args(["diff", "--name-only", "--diff-filter=M", "HEAD"])
        .current_dir(project_root)
        .output()
        .with_context(|| {
            format!(
                "Failed to inspect repo-tracked mutations in {}",
                project_root.display()
            )
        })?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8(output.stdout).context("git diff output was not valid UTF-8")?;
    let mut mutated = Vec::new();
    for rel_path in stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let candidate = project_root.join(rel_path);
        let Ok(metadata) = fs::metadata(&candidate) else {
            continue;
        };
        let Ok(modified_at) = metadata.modified() else {
            continue;
        };
        if modified_at > session_start_time {
            mutated.push(PathBuf::from(rel_path));
        }
    }

    Ok(mutated)
}

pub fn write_audit_warning_artifact(
    session_dir: &Path,
    mutated_paths: &[PathBuf],
) -> Result<Option<PathBuf>> {
    if mutated_paths.is_empty() {
        return Ok(None);
    }

    let output_dir = session_dir.join("output");
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("Failed to create output dir: {}", output_dir.display()))?;
    let artifact_path = output_dir.join("audit-warnings.md");
    let mut body = String::from(
        "# Audit Warnings\n\nRepo-tracked files mutated during a read-only/recon-style session:\n",
    );
    for path in mutated_paths {
        body.push_str("- `");
        body.push_str(&path.display().to_string());
        body.push_str("`\n");
    }
    fs::write(&artifact_path, body).with_context(|| {
        format!(
            "Failed to write audit warnings: {}",
            artifact_path.display()
        )
    })?;
    Ok(Some(artifact_path))
}
