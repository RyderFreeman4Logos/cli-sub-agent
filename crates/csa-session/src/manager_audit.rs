use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoWriteAudit {
    pub added: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
    pub renamed: Vec<(PathBuf, PathBuf)>,
}

impl RepoWriteAudit {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.modified.is_empty()
            && self.deleted.is_empty()
            && self.renamed.is_empty()
    }
}

#[derive(Debug, Default)]
struct RepoWriteAuditSets {
    added: BTreeSet<PathBuf>,
    modified: BTreeSet<PathBuf>,
    deleted: BTreeSet<PathBuf>,
    renamed: BTreeSet<(PathBuf, PathBuf)>,
}

impl RepoWriteAuditSets {
    fn finish(self) -> RepoWriteAudit {
        RepoWriteAudit {
            added: self.added.into_iter().collect(),
            modified: self.modified.into_iter().collect(),
            deleted: self.deleted.into_iter().collect(),
            renamed: self.renamed.into_iter().collect(),
        }
    }
}

pub fn compute_repo_write_audit(
    project_root: &Path,
    pre_session_head: &str,
    pre_session_porcelain: Option<&str>,
) -> Result<RepoWriteAudit> {
    let mut audit = RepoWriteAuditSets::default();
    collect_committed_changes(project_root, pre_session_head, &mut audit)?;
    collect_uncommitted_changes(project_root, pre_session_porcelain, &mut audit)?;
    Ok(audit.finish())
}

fn collect_committed_changes(
    project_root: &Path,
    pre_session_head: &str,
    audit: &mut RepoWriteAuditSets,
) -> Result<()> {
    let revision_range = format!("{pre_session_head}..HEAD");
    let output = Command::new("git")
        .args(["diff", "--name-status", "-z", &revision_range])
        .current_dir(project_root)
        .output()
        .with_context(|| {
            format!(
                "Failed to inspect committed repo-tracked mutations in {}",
                project_root.display()
            )
        })?;

    if !output.status.success() {
        return Ok(());
    }

    let mut fields = output.stdout.split(|byte| *byte == 0).peekable();
    while let Some(status) = next_non_empty_field(&mut fields) {
        let status = std::str::from_utf8(status).context("git diff status was not valid UTF-8")?;
        let kind = status.chars().next().unwrap_or_default();
        match kind {
            'A' => {
                if let Some(path) = next_non_empty_path(&mut fields)? {
                    audit.added.insert(path);
                }
            }
            'M' | 'T' => {
                if let Some(path) = next_non_empty_path(&mut fields)? {
                    audit.modified.insert(path);
                }
            }
            'D' => {
                if let Some(path) = next_non_empty_path(&mut fields)? {
                    audit.deleted.insert(path);
                }
            }
            'R' => {
                let Some(old_path) = next_non_empty_path(&mut fields)? else {
                    continue;
                };
                let Some(new_path) = next_non_empty_path(&mut fields)? else {
                    continue;
                };
                audit.renamed.insert((old_path, new_path));
            }
            'C' => {
                let _ = next_non_empty_path(&mut fields)?;
                if let Some(new_path) = next_non_empty_path(&mut fields)? {
                    audit.added.insert(new_path);
                }
            }
            _ => {
                if let Some(path) = next_non_empty_path(&mut fields)? {
                    audit.modified.insert(path);
                }
            }
        }
    }

    Ok(())
}

fn collect_uncommitted_changes(
    project_root: &Path,
    pre_session_porcelain: Option<&str>,
    audit: &mut RepoWriteAuditSets,
) -> Result<()> {
    let output = Command::new("git")
        .args(["status", "--porcelain=v1", "-z"])
        .current_dir(project_root)
        .output()
        .with_context(|| {
            format!(
                "Failed to inspect uncommitted repo-tracked mutations in {}",
                project_root.display()
            )
        })?;

    if !output.status.success() {
        return Ok(());
    }

    let post_entries = parse_porcelain_entries(&output.stdout)?;
    let Some(pre_snapshot) = pre_session_porcelain else {
        tracing::warn!(
            project_root = %project_root.display(),
            "repo-write audit missing pre-session porcelain snapshot; falling back to commits-only attribution for uncommitted changes"
        );
        return Ok(());
    };

    let pre_entries = parse_porcelain_entries(pre_snapshot.as_bytes())?;
    let new_entries = diff_porcelain_entries(&pre_entries, &post_entries);
    if has_same_status_overlap(&pre_entries, &post_entries) {
        tracing::info!(
            project_root = %project_root.display(),
            "repo-write audit conservatively ignores files dirty before and after the session when porcelain status is unchanged"
        );
    }

    for entry in new_entries.values() {
        apply_porcelain_entry(entry, audit);
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PorcelainStatus {
    x: char,
    y: char,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PorcelainEntry {
    status: PorcelainStatus,
    path: PathBuf,
    source_path: Option<PathBuf>,
}

impl PorcelainEntry {
    fn key(&self) -> (PathBuf, PorcelainStatus) {
        (self.path.clone(), self.status)
    }
}

fn parse_porcelain_entries(
    raw: &[u8],
) -> Result<BTreeMap<(PathBuf, PorcelainStatus), PorcelainEntry>> {
    let mut entries = BTreeMap::new();
    let mut fields = raw.split(|byte| *byte == 0).peekable();
    while let Some(entry) = next_non_empty_field(&mut fields) {
        if entry.len() < 4 {
            continue;
        }
        let x = entry[0] as char;
        let y = entry[1] as char;
        if x == '?' && y == '?' {
            continue;
        }

        let path = bytes_to_path(&entry[3..])?;
        let source_path = if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
            next_non_empty_path(&mut fields)?
        } else {
            None
        };
        let parsed = PorcelainEntry {
            status: PorcelainStatus { x, y },
            path,
            source_path,
        };
        entries.insert(parsed.key(), parsed);
    }
    Ok(entries)
}

fn diff_porcelain_entries(
    pre_entries: &BTreeMap<(PathBuf, PorcelainStatus), PorcelainEntry>,
    post_entries: &BTreeMap<(PathBuf, PorcelainStatus), PorcelainEntry>,
) -> BTreeMap<(PathBuf, PorcelainStatus), PorcelainEntry> {
    post_entries
        .iter()
        .filter(|(key, _)| !pre_entries.contains_key(*key))
        .map(|(key, entry)| (key.clone(), entry.clone()))
        .collect()
}

fn has_same_status_overlap(
    pre_entries: &BTreeMap<(PathBuf, PorcelainStatus), PorcelainEntry>,
    post_entries: &BTreeMap<(PathBuf, PorcelainStatus), PorcelainEntry>,
) -> bool {
    post_entries.keys().any(|key| pre_entries.contains_key(key))
}

fn apply_porcelain_entry(entry: &PorcelainEntry, audit: &mut RepoWriteAuditSets) {
    let x = entry.status.x;
    let y = entry.status.y;

    if let Some(source_path) = entry.source_path.as_ref() {
        if x == 'R' || y == 'R' {
            audit
                .renamed
                .insert((source_path.clone(), entry.path.clone()));
        } else {
            audit.added.insert(entry.path.clone());
        }
    }

    if x == 'A' || y == 'A' {
        audit.added.insert(entry.path.clone());
    }
    if x == 'D' || y == 'D' {
        audit.deleted.insert(entry.path.clone());
    }
    if path_is_modified(x, y) {
        audit.modified.insert(entry.path.clone());
    }
}

fn path_is_modified(x: char, y: char) -> bool {
    matches!(x, 'M' | 'T' | 'U') || matches!(y, 'M' | 'T' | 'U')
}

fn next_non_empty_field<'a, I>(fields: &mut std::iter::Peekable<I>) -> Option<&'a [u8]>
where
    I: Iterator<Item = &'a [u8]>,
{
    fields.by_ref().find(|field| !field.is_empty())
}

fn next_non_empty_path<'a, I>(fields: &mut std::iter::Peekable<I>) -> Result<Option<PathBuf>>
where
    I: Iterator<Item = &'a [u8]>,
{
    next_non_empty_field(fields).map(bytes_to_path).transpose()
}

fn bytes_to_path(bytes: &[u8]) -> Result<PathBuf> {
    let path = std::str::from_utf8(bytes).context("git path output was not valid UTF-8")?;
    Ok(PathBuf::from(path))
}

pub fn write_audit_warning_artifact(
    session_dir: &Path,
    audit: &RepoWriteAudit,
) -> Result<Option<PathBuf>> {
    if audit.is_empty() {
        return Ok(None);
    }

    let output_dir = session_dir.join("output");
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("Failed to create output dir: {}", output_dir.display()))?;
    let artifact_path = output_dir.join("audit-warnings.md");
    let mut body = String::from(
        "# Audit Warnings\n\nRepo-tracked files mutated during a read-only/recon-style session.\n",
    );
    append_path_section(&mut body, "Added", &audit.added);
    append_path_section(&mut body, "Modified", &audit.modified);
    append_path_section(&mut body, "Deleted", &audit.deleted);
    append_rename_section(&mut body, &audit.renamed);
    fs::write(&artifact_path, body).with_context(|| {
        format!(
            "Failed to write audit warnings: {}",
            artifact_path.display()
        )
    })?;
    Ok(Some(artifact_path))
}

fn append_path_section(body: &mut String, heading: &str, paths: &[PathBuf]) {
    if paths.is_empty() {
        return;
    }
    body.push_str("\n## ");
    body.push_str(heading);
    body.push('\n');
    for path in paths {
        body.push_str("- `");
        body.push_str(&path.display().to_string());
        body.push_str("`\n");
    }
}

fn append_rename_section(body: &mut String, renames: &[(PathBuf, PathBuf)]) {
    if renames.is_empty() {
        return;
    }
    body.push_str("\n## Renamed\n");
    for (old_path, new_path) in renames {
        body.push_str("- `");
        body.push_str(&old_path.display().to_string());
        body.push_str("` -> `");
        body.push_str(&new_path.display().to_string());
        body.push_str("`\n");
    }
}
