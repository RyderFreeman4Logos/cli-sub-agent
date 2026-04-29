use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use csa_core::checklist::{CheckStatus, ChecklistDocument};
use fd_lock::RwLock;
use sha2::{Digest, Sha256};

pub struct ChecklistStore {
    base_dir: PathBuf,
}

impl ChecklistStore {
    pub fn new() -> Result<Self> {
        let state_dir =
            csa_config::paths::state_dir_write().context("Failed to determine state directory")?;
        Ok(Self {
            base_dir: state_dir.join("checklists"),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_base_dir(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    pub fn project_dir(&self, project_root: &Path) -> PathBuf {
        self.base_dir.join(project_hash(project_root))
    }

    pub fn branch_dir(&self, project_root: &Path, branch: &str) -> PathBuf {
        self.project_dir(project_root)
            .join(branch_storage_path(branch))
    }

    pub fn checklist_path(&self, project_root: &Path, branch: &str) -> PathBuf {
        self.branch_dir(project_root, branch).join("checklist.toml")
    }

    pub fn load(&self, project_root: &Path, branch: &str) -> Result<Option<ChecklistDocument>> {
        let branch_dir = self.branch_dir(project_root, branch);
        let lock_path = branch_dir.join("checklist.lock");
        let checklist_path = branch_dir.join("checklist.toml");
        if !checklist_path.exists() {
            return Ok(None);
        }

        fs::create_dir_all(&branch_dir)
            .with_context(|| format!("Failed to create checklist dir: {}", branch_dir.display()))?;
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("Failed to open checklist lock: {}", lock_path.display()))?;
        let mut lock = RwLock::new(lock_file);
        let _guard = lock
            .write()
            .with_context(|| format!("Failed to lock checklist file: {}", lock_path.display()))?;

        read_document(&checklist_path).map(Some)
    }

    pub fn save(&self, project_root: &Path, branch: &str, doc: &ChecklistDocument) -> Result<()> {
        let branch_dir = self.branch_dir(project_root, branch);
        fs::create_dir_all(&branch_dir)
            .with_context(|| format!("Failed to create checklist dir: {}", branch_dir.display()))?;
        let lock_path = branch_dir.join("checklist.lock");
        let checklist_path = branch_dir.join("checklist.toml");

        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("Failed to open checklist lock: {}", lock_path.display()))?;
        let mut lock = RwLock::new(lock_file);
        let _guard = lock
            .write()
            .with_context(|| format!("Failed to lock checklist file: {}", lock_path.display()))?;

        write_document(&checklist_path, doc)
    }

    pub fn check_item(
        &self,
        project_root: &Path,
        branch: &str,
        item_id: &str,
        evidence: &str,
        reviewer: &str,
    ) -> Result<()> {
        self.update_item(project_root, branch, item_id, |item| {
            item.status = CheckStatus::Checked;
            item.evidence = evidence.to_string();
            item.reviewer = reviewer.to_string();
            item.checked_at = chrono::Utc::now().to_rfc3339();
        })
    }

    pub fn reset_item(&self, project_root: &Path, branch: &str, item_id: &str) -> Result<()> {
        self.update_item(project_root, branch, item_id, |item| {
            item.status = CheckStatus::Unchecked;
            item.evidence.clear();
            item.reviewer.clear();
            item.checked_at.clear();
        })
    }

    pub fn list_projects(&self) -> Result<Vec<PathBuf>> {
        let entries = match fs::read_dir(&self.base_dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("Failed to read checklist dir: {}", self.base_dir.display())
                });
            }
        };

        let mut projects = Vec::new();
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                projects.push(entry.path());
            }
        }
        projects.sort();
        Ok(projects)
    }

    fn update_item<F>(
        &self,
        project_root: &Path,
        branch: &str,
        item_id: &str,
        update: F,
    ) -> Result<()>
    where
        F: FnOnce(&mut csa_core::checklist::ChecklistItem),
    {
        let branch_dir = self.branch_dir(project_root, branch);
        fs::create_dir_all(&branch_dir)
            .with_context(|| format!("Failed to create checklist dir: {}", branch_dir.display()))?;
        let lock_path = branch_dir.join("checklist.lock");
        let checklist_path = branch_dir.join("checklist.toml");
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("Failed to open checklist lock: {}", lock_path.display()))?;
        let mut lock = RwLock::new(lock_file);
        let _guard = lock
            .write()
            .with_context(|| format!("Failed to lock checklist file: {}", lock_path.display()))?;

        let mut doc = read_document(&checklist_path)?;
        let Some(item) = doc.criteria.iter_mut().find(|item| item.id == item_id) else {
            bail!("Checklist criterion not found: {item_id}");
        };
        update(item);
        write_document(&checklist_path, &doc)
    }
}

fn project_hash(project_root: &Path) -> String {
    let normalized = fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(normalized.to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn branch_storage_path(branch: &str) -> PathBuf {
    let mut path = PathBuf::new();
    for segment in branch.replace('\\', "/").split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            path.push("_");
        } else {
            path.push(segment);
        }
    }
    if path.as_os_str().is_empty() {
        path.push("_");
    }
    path
}

fn read_document(path: &Path) -> Result<ChecklistDocument> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read checklist: {}", path.display()))?;
    toml::from_str(&content)
        .with_context(|| format!("Failed to parse checklist: {}", path.display()))
}

fn write_document(path: &Path, doc: &ChecklistDocument) -> Result<()> {
    let content = toml::to_string_pretty(doc).context("Failed to serialize checklist")?;
    let tmp_path = path.with_extension("toml.tmp");
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)
            .with_context(|| {
                format!("Failed to open checklist temp file: {}", tmp_path.display())
            })?;
        file.write_all(content.as_bytes()).with_context(|| {
            format!(
                "Failed to write checklist temp file: {}",
                tmp_path.display()
            )
        })?;
        file.sync_all().with_context(|| {
            format!("Failed to sync checklist temp file: {}", tmp_path.display())
        })?;
    }
    fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "Failed to replace checklist {} with {}",
            path.display(),
            tmp_path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::ChecklistStore;
    use csa_core::checklist::{
        CheckStatus, ChecklistDocument, ChecklistItem, ChecklistMeta, ChecklistSummary,
    };
    use tempfile::TempDir;

    fn document(project_root: &str, branch: &str) -> ChecklistDocument {
        ChecklistDocument {
            meta: ChecklistMeta {
                project_root: project_root.to_string(),
                branch: branch.to_string(),
                created_at: "2026-04-28T00:00:00Z".to_string(),
                scope: "base:main".to_string(),
                profile: "rust".to_string(),
            },
            criteria: vec![ChecklistItem {
                id: "rust-002".to_string(),
                source: "Rust 002".to_string(),
                description: "Errors propagate without unwrap".to_string(),
                status: CheckStatus::Unchecked,
                evidence: String::new(),
                reviewer: String::new(),
                checked_at: String::new(),
            }],
        }
    }

    #[test]
    fn save_load_roundtrip() {
        let tmp = TempDir::new().expect("tempdir");
        let store = ChecklistStore::with_base_dir(tmp.path().join("checklists"));
        let doc = document(tmp.path().to_str().expect("utf8 path"), "feature");

        store.save(tmp.path(), "feature", &doc).expect("save");
        let loaded = store.load(tmp.path(), "feature").expect("load");

        assert_eq!(loaded, Some(doc));
    }

    #[test]
    fn check_and_reset_item_update_document() {
        let tmp = TempDir::new().expect("tempdir");
        let store = ChecklistStore::with_base_dir(tmp.path().join("checklists"));
        let doc = document(tmp.path().to_str().expect("utf8 path"), "feature");
        store.save(tmp.path(), "feature", &doc).expect("save");

        store
            .check_item(tmp.path(), "feature", "rust-002", "cargo test", "codex")
            .expect("check item");
        let checked = store
            .load(tmp.path(), "feature")
            .expect("load")
            .expect("document exists");
        assert_eq!(
            checked.summary(),
            ChecklistSummary {
                unchecked: 0,
                checked: 1,
                failed: 0,
                not_applicable: 0,
            }
        );
        assert_eq!(checked.criteria[0].evidence, "cargo test");
        assert_eq!(checked.criteria[0].reviewer, "codex");
        assert!(!checked.criteria[0].checked_at.is_empty());

        store
            .reset_item(tmp.path(), "feature", "rust-002")
            .expect("reset item");
        let reset = store
            .load(tmp.path(), "feature")
            .expect("load")
            .expect("document exists");
        assert_eq!(reset.criteria[0].status, CheckStatus::Unchecked);
        assert!(reset.criteria[0].evidence.is_empty());
        assert!(reset.criteria[0].reviewer.is_empty());
        assert!(reset.criteria[0].checked_at.is_empty());
    }

    #[test]
    fn branch_dir_preserves_slash_branches_without_path_traversal() {
        let tmp = TempDir::new().expect("tempdir");
        let store = ChecklistStore::with_base_dir(tmp.path().join("checklists"));

        let slash_branch = store.branch_dir(tmp.path(), "feat/1199-review-checklist-notebook");
        assert!(slash_branch.ends_with("feat/1199-review-checklist-notebook"));

        let traversal_branch = store.branch_dir(tmp.path(), "../outside");
        assert!(traversal_branch.starts_with(store.project_dir(tmp.path())));
        assert!(traversal_branch.ends_with("_/outside"));
    }
}
