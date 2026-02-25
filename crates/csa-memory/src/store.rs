use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use regex::RegexBuilder;
use tracing::warn;

use crate::entry::MemoryEntry;

const MEMORY_FILE_NAME: &str = "memories.jsonl";
const APP_NAME: &str = "cli-sub-agent";

#[derive(Debug, Clone, Default)]
pub struct MemoryFilter {
    pub project: Option<String>,
    pub tool: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub tag: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MemoryStore {
    base_dir: PathBuf,
    file_path: PathBuf,
}

impl MemoryStore {
    pub fn new(base_dir: PathBuf) -> Self {
        let base_dir = if base_dir.as_os_str().is_empty() {
            default_memory_base_dir()
        } else {
            base_dir
        };
        Self {
            file_path: base_dir.join(MEMORY_FILE_NAME),
            base_dir,
        }
    }

    pub fn append(&self, entry: &MemoryEntry) -> Result<()> {
        self.ensure_storage_dir()?;

        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.file_path)
            .with_context(|| format!("failed to open memory file: {}", self.file_path.display()))?;

        set_file_mode_600(&self.file_path)?;

        let line = serde_json::to_string(entry).context("failed to serialize memory entry")?;
        writeln!(file, "{line}").context("failed to append memory entry")?;
        file.flush()
            .context("failed to flush memory entry append")?;

        Ok(())
    }

    /// Rewrite all entries atomically (used by consolidation/GC).
    pub fn rewrite_all(&self, entries: &[MemoryEntry]) -> Result<()> {
        self.ensure_storage_dir()?;

        let tmp_path = self.base_dir.join("memories.jsonl.tmp");
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp_path)
            .with_context(|| format!("failed to open temp memory file: {}", tmp_path.display()))?;
        set_file_mode_600(&tmp_path)?;

        let mut writer = BufWriter::new(file);
        for entry in entries {
            let line = serde_json::to_string(entry).context("failed to serialize memory entry")?;
            writeln!(writer, "{line}").context("failed to write memory entry")?;
        }
        writer
            .flush()
            .context("failed to flush rewritten memory file")?;

        fs::rename(&tmp_path, &self.file_path).with_context(|| {
            format!(
                "failed to atomically replace memory file {}",
                self.file_path.display()
            )
        })?;
        Ok(())
    }

    pub fn load_all(&self) -> Result<Vec<MemoryEntry>> {
        if !self.file_path.exists() {
            return Ok(Vec::new());
        }

        let file = OpenOptions::new()
            .read(true)
            .open(&self.file_path)
            .with_context(|| format!("failed to read memory file: {}", self.file_path.display()))?;
        let reader = BufReader::new(file);

        let now = Utc::now();
        let mut entries = Vec::new();
        for (idx, line_result) in reader.lines().enumerate() {
            let line = line_result.with_context(|| {
                format!(
                    "failed to read memory line {} from {}",
                    idx + 1,
                    self.file_path.display()
                )
            })?;

            if line.trim().is_empty() {
                continue;
            }

            match serde_json::from_str::<MemoryEntry>(&line) {
                Ok(entry) => {
                    if entry.valid_until.is_some_and(|until| until < now) {
                        continue;
                    }
                    entries.push(entry);
                }
                Err(error) => {
                    warn!(
                        path = %self.file_path.display(),
                        line_number = idx + 1,
                        %error,
                        "skipping corrupt memory jsonl line"
                    );
                }
            }
        }

        Ok(entries)
    }

    pub fn quick_search(&self, pattern: &str) -> Result<Vec<MemoryEntry>> {
        let regex = RegexBuilder::new(pattern)
            .case_insensitive(true)
            .build()
            .with_context(|| format!("invalid regex pattern: {pattern}"))?;

        let mut entries: Vec<MemoryEntry> = self
            .load_all()?
            .into_iter()
            .filter(|entry| {
                regex.is_match(&entry.content)
                    || entry.facts.iter().any(|fact| regex.is_match(fact))
            })
            .collect();

        entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        Ok(entries)
    }

    pub fn list(&self, filter: MemoryFilter) -> Result<Vec<MemoryEntry>> {
        let mut entries: Vec<MemoryEntry> = self
            .load_all()?
            .into_iter()
            .filter(|entry| match &filter.project {
                Some(project) => entry.project.as_ref() == Some(project),
                None => true,
            })
            .filter(|entry| match &filter.tool {
                Some(tool) => entry.tool.as_ref() == Some(tool),
                None => true,
            })
            .filter(|entry| match filter.since {
                Some(since) => entry.timestamp >= since,
                None => true,
            })
            .filter(|entry| match &filter.tag {
                Some(tag) => entry.tags.iter().any(|entry_tag| entry_tag == tag),
                None => true,
            })
            .collect();

        entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        Ok(entries)
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    fn ensure_storage_dir(&self) -> Result<()> {
        let dir_exists = self.base_dir.exists();
        fs::create_dir_all(&self.base_dir)
            .with_context(|| format!("failed to create memory dir: {}", self.base_dir.display()))?;

        if !dir_exists {
            set_dir_mode_700(&self.base_dir)?;
        }

        Ok(())
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new(PathBuf::new())
    }
}

pub fn append_entry(entry: &MemoryEntry) -> Result<()> {
    MemoryStore::default().append(entry)
}

pub fn quick_search(pattern: &str) -> Result<Vec<MemoryEntry>> {
    MemoryStore::default().quick_search(pattern)
}

pub fn list_entries(filter: MemoryFilter) -> Result<Vec<MemoryEntry>> {
    MemoryStore::default().list(filter)
}

fn default_memory_base_dir() -> PathBuf {
    if let Some(project_dirs) = directories::ProjectDirs::from("", "", APP_NAME) {
        return project_dirs
            .state_dir()
            .unwrap_or_else(|| project_dirs.data_local_dir())
            .join("memory");
    }

    if let Some(base_dirs) = directories::BaseDirs::new() {
        return base_dirs
            .home_dir()
            .join(".local")
            .join("state")
            .join(APP_NAME)
            .join("memory");
    }

    std::env::temp_dir()
        .join(format!("{APP_NAME}-state"))
        .join("memory")
}

#[cfg(unix)]
fn set_dir_mode_700(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed to chmod 700: {}", path.display()))
}

#[cfg(not(unix))]
fn set_dir_mode_700(_path: &std::path::Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_file_mode_600(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to chmod 600: {}", path.display()))
}

#[cfg(not(unix))]
fn set_file_mode_600(_path: &std::path::Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::MemorySource;
    use chrono::Duration;
    use ulid::Ulid;

    fn make_test_store() -> MemoryStore {
        let dir = std::env::temp_dir().join(format!("csa-memory-test-{}", Ulid::new()));
        MemoryStore::new(dir)
    }

    fn make_entry(
        content: &str,
        project: Option<&str>,
        tool: Option<&str>,
        tags: &[&str],
        facts: &[&str],
        valid_until: Option<DateTime<Utc>>,
    ) -> MemoryEntry {
        MemoryEntry {
            id: Ulid::new(),
            timestamp: Utc::now(),
            project: project.map(str::to_string),
            tool: tool.map(str::to_string),
            session_id: Some(format!("session-{}", Ulid::new())),
            tags: tags.iter().map(|tag| (*tag).to_string()).collect(),
            content: content.to_string(),
            facts: facts.iter().map(|fact| (*fact).to_string()).collect(),
            source: MemorySource::PostRun,
            valid_from: None,
            valid_until,
        }
    }

    #[test]
    fn test_append_and_load() {
        let store = make_test_store();

        let e1 = make_entry("first", Some("proj-a"), Some("codex"), &["a"], &[], None);
        let e2 = make_entry("second", Some("proj-a"), Some("codex"), &["b"], &[], None);
        let e3 = make_entry("third", Some("proj-b"), Some("claude"), &["c"], &[], None);

        store.append(&e1).unwrap();
        store.append(&e2).unwrap();
        store.append(&e3).unwrap();

        let all = store.load_all().unwrap();
        assert_eq!(all.len(), 3);
        assert!(all.iter().any(|entry| entry.content == "first"));
        assert!(all.iter().any(|entry| entry.content == "second"));
        assert!(all.iter().any(|entry| entry.content == "third"));
    }

    #[test]
    fn test_quick_search() {
        let store = make_test_store();

        store
            .append(&make_entry(
                "Rust ownership issue",
                Some("proj-a"),
                Some("codex"),
                &["rust"],
                &["borrow checker"],
                None,
            ))
            .unwrap();
        store
            .append(&make_entry(
                "Session orchestration",
                Some("proj-b"),
                Some("claude"),
                &["ops"],
                &["tool failover"],
                None,
            ))
            .unwrap();
        store
            .append(&make_entry(
                "Memory system design",
                Some("proj-a"),
                Some("codex"),
                &["design"],
                &["Rust module"],
                None,
            ))
            .unwrap();

        let matched = store.quick_search("rust").unwrap();
        assert_eq!(matched.len(), 2);
        assert!(matched.iter().all(|entry| {
            entry.content.to_lowercase().contains("rust")
                || entry
                    .facts
                    .iter()
                    .any(|fact| fact.to_lowercase().contains("rust"))
        }));
    }

    #[test]
    fn test_list_filter_by_project() {
        let store = make_test_store();
        store
            .append(&make_entry(
                "a",
                Some("proj-a"),
                Some("codex"),
                &["x"],
                &[],
                None,
            ))
            .unwrap();
        store
            .append(&make_entry(
                "b",
                Some("proj-b"),
                Some("codex"),
                &["x"],
                &[],
                None,
            ))
            .unwrap();

        let entries = store
            .list(MemoryFilter {
                project: Some("proj-a".to_string()),
                ..MemoryFilter::default()
            })
            .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].project.as_deref(), Some("proj-a"));
    }

    #[test]
    fn test_list_filter_by_tool() {
        let store = make_test_store();
        store
            .append(&make_entry(
                "a",
                Some("proj-a"),
                Some("codex"),
                &["x"],
                &[],
                None,
            ))
            .unwrap();
        store
            .append(&make_entry(
                "b",
                Some("proj-a"),
                Some("gemini"),
                &["x"],
                &[],
                None,
            ))
            .unwrap();

        let entries = store
            .list(MemoryFilter {
                tool: Some("codex".to_string()),
                ..MemoryFilter::default()
            })
            .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].tool.as_deref(), Some("codex"));
    }

    #[test]
    fn test_list_filter_by_tag() {
        let store = make_test_store();
        store
            .append(&make_entry(
                "a",
                Some("proj-a"),
                Some("codex"),
                &["security", "urgent"],
                &[],
                None,
            ))
            .unwrap();
        store
            .append(&make_entry(
                "b",
                Some("proj-a"),
                Some("codex"),
                &["feature"],
                &[],
                None,
            ))
            .unwrap();

        let entries = store
            .list(MemoryFilter {
                tag: Some("security".to_string()),
                ..MemoryFilter::default()
            })
            .unwrap();

        assert_eq!(entries.len(), 1);
        assert!(entries[0].tags.iter().any(|tag| tag == "security"));
    }

    #[test]
    fn test_expired_entries_skipped() {
        let store = make_test_store();

        let expired = make_entry(
            "expired",
            Some("proj-a"),
            Some("codex"),
            &["old"],
            &[],
            Some(Utc::now() - Duration::minutes(5)),
        );
        let active = make_entry("active", Some("proj-a"), Some("codex"), &["new"], &[], None);

        store.append(&expired).unwrap();
        store.append(&active).unwrap();

        let entries = store.load_all().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, "active");
    }

    #[test]
    fn test_corrupt_line_tolerance() {
        let store = make_test_store();
        let valid_one = make_entry("valid-one", Some("proj-a"), Some("codex"), &[], &[], None);
        let valid_two = make_entry("valid-two", Some("proj-a"), Some("codex"), &[], &[], None);

        store.append(&valid_one).unwrap();

        {
            let mut file = OpenOptions::new()
                .append(true)
                .open(&store.file_path)
                .unwrap();
            writeln!(file, "{{ this is not valid json").unwrap();
        }

        store.append(&valid_two).unwrap();

        let entries = store.load_all().unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|entry| entry.content == "valid-one"));
        assert!(entries.iter().any(|entry| entry.content == "valid-two"));
    }

    #[test]
    fn test_rewrite_all() {
        let store = make_test_store();

        let old_one = make_entry("old-one", Some("proj-a"), Some("codex"), &[], &[], None);
        let old_two = make_entry("old-two", Some("proj-a"), Some("codex"), &[], &[], None);
        store.append(&old_one).unwrap();
        store.append(&old_two).unwrap();
        assert_eq!(store.load_all().unwrap().len(), 2);

        let replacement = make_entry("replacement", Some("proj-b"), Some("codex"), &[], &[], None);
        store
            .rewrite_all(std::slice::from_ref(&replacement))
            .unwrap();

        let entries = store.load_all().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content, "replacement");
        assert_eq!(entries[0].project.as_deref(), Some("proj-b"));
    }
}
