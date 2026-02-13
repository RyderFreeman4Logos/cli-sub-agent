//! TODO plan management for CSA projects.
//!
//! Each project has a todos directory at `~/.local/state/csa/{project_path}/todos/`,
//! organized as a single git repository tracking all plans as timestamped subdirectories.
//!
//! # Directory Layout
//!
//! ```text
//! ~/.local/state/csa/{project_path}/todos/
//! ├── .git/              (single git repo, managed by `csa todo save/history`)
//! ├── .lock              (flock for concurrent write protection)
//! ├── 20260211T023000/
//! │   ├── metadata.toml
//! │   └── TODO.md
//! └── 20260211T143000/
//!     ├── metadata.toml
//!     └── TODO.md
//! ```

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const METADATA_FILE: &str = "metadata.toml";
const TODO_MD_FILE: &str = "TODO.md";
const LOCK_FILE: &str = ".lock";

/// Validate a timestamp string to prevent path traversal.
///
/// Accepts: `20260211T023000` or `20260211T023000-1` (collision suffix).
/// Rejects: `../x`, `/tmp/x`, `a/b`, empty, etc.
pub(crate) fn validate_timestamp(timestamp: &str) -> Result<()> {
    if timestamp.is_empty() {
        anyhow::bail!("Timestamp must not be empty");
    }

    // Reject path separators and traversal components
    if timestamp.contains('/') || timestamp.contains('\\') || timestamp.contains("..") {
        anyhow::bail!("Invalid timestamp (path traversal detected): '{timestamp}'");
    }

    // Whitelist: digits, 'T', '-' only
    if !timestamp
        .chars()
        .all(|c| c.is_ascii_digit() || c == 'T' || c == '-')
    {
        anyhow::bail!("Invalid timestamp format: '{timestamp}'. Expected: YYYYMMDDTHHMMSS[-N]");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Status of a TODO plan through its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TodoStatus {
    Draft,
    Debating,
    Approved,
    Implementing,
    Done,
}

impl std::fmt::Display for TodoStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Draft => write!(f, "draft"),
            Self::Debating => write!(f, "debating"),
            Self::Approved => write!(f, "approved"),
            Self::Implementing => write!(f, "implementing"),
            Self::Done => write!(f, "done"),
        }
    }
}

impl std::str::FromStr for TodoStatus {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "draft" => Ok(Self::Draft),
            "debating" => Ok(Self::Debating),
            "approved" => Ok(Self::Approved),
            "implementing" => Ok(Self::Implementing),
            "done" => Ok(Self::Done),
            _ => anyhow::bail!(
                "Invalid TODO status: '{s}'. Valid: draft, debating, approved, implementing, done"
            ),
        }
    }
}

/// Metadata for a single TODO plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoMetadata {
    /// Git branch this plan is associated with.
    pub branch: Option<String>,
    /// Current lifecycle status.
    pub status: TodoStatus,
    /// Human-readable title.
    pub title: String,
    /// CSA session IDs linked to this plan.
    pub sessions: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A TODO plan with its filesystem location and metadata.
#[derive(Debug, Clone)]
pub struct TodoPlan {
    /// Timestamp identifier (e.g., `20260211T023000`).
    pub timestamp: String,
    /// Absolute path to the plan subdirectory.
    pub todo_dir: PathBuf,
    /// Deserialized metadata.
    pub metadata: TodoMetadata,
}

impl TodoPlan {
    pub fn metadata_path(&self) -> PathBuf {
        self.todo_dir.join(METADATA_FILE)
    }

    pub fn todo_md_path(&self) -> PathBuf {
        self.todo_dir.join(TODO_MD_FILE)
    }
}

// ---------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------

/// Manages TODO plans under a project's todos directory.
///
/// All write operations are protected by an `flock` on `.lock` in the todos
/// directory, and individual file writes use temp-file + rename for atomicity.
pub struct TodoManager {
    todos_dir: PathBuf,
}

impl TodoManager {
    /// Create a manager for the given project path.
    ///
    /// Resolves the todos directory via `csa-session`'s project root:
    /// `~/.local/state/csa/{encoded_project_path}/todos/`
    pub fn new(project_path: &Path) -> Result<Self> {
        let session_root = csa_session::manager::get_session_root(project_path)?;
        Ok(Self {
            todos_dir: session_root.join("todos"),
        })
    }

    /// Create a manager with an explicit base directory (for testing).
    pub fn with_base_dir(base_dir: PathBuf) -> Self {
        Self {
            todos_dir: base_dir,
        }
    }

    /// Get the todos directory path.
    pub fn todos_dir(&self) -> &Path {
        &self.todos_dir
    }

    // -- Write operations (flock-protected) --------------------------------

    /// Create a new TODO plan.
    pub fn create(&self, title: &str, branch: Option<&str>) -> Result<TodoPlan> {
        self.with_write_lock(|| self.create_inner(title, branch))
    }

    /// Update the status of a TODO plan.
    pub fn update_status(&self, timestamp: &str, status: TodoStatus) -> Result<TodoPlan> {
        self.with_write_lock(|| {
            let mut plan = self.load_inner(timestamp)?;
            plan.metadata.status = status;
            plan.metadata.updated_at = Utc::now();
            self.write_metadata(&plan)?;
            Ok(plan)
        })
    }

    /// Link a CSA session ID to a TODO plan (idempotent).
    pub fn link_session(&self, timestamp: &str, session_id: &str) -> Result<TodoPlan> {
        self.with_write_lock(|| {
            let mut plan = self.load_inner(timestamp)?;
            if !plan.metadata.sessions.contains(&session_id.to_string()) {
                plan.metadata.sessions.push(session_id.to_string());
                plan.metadata.updated_at = Utc::now();
                self.write_metadata(&plan)?;
            }
            Ok(plan)
        })
    }

    /// Write (overwrite) the TODO.md content for a plan.
    ///
    /// Also updates `updated_at` in metadata to reflect the content change.
    pub fn write_todo_md(&self, timestamp: &str, content: &str) -> Result<()> {
        self.with_write_lock(|| {
            let mut plan = self.load_inner(timestamp)?;
            atomic_write(&plan.todo_md_path(), content.as_bytes())?;
            plan.metadata.updated_at = Utc::now();
            self.write_metadata(&plan)
        })
    }

    // -- Read operations (no lock needed) ----------------------------------

    /// Load a TODO plan by timestamp.
    pub fn load(&self, timestamp: &str) -> Result<TodoPlan> {
        self.load_inner(timestamp)
    }

    /// Load the most recent TODO plan, or error if none exist.
    pub fn latest(&self) -> Result<TodoPlan> {
        let plans = self.list()?;
        plans
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No TODO plans found for this project"))
    }

    /// List all TODO plans for this project, sorted newest-first.
    pub fn list(&self) -> Result<Vec<TodoPlan>> {
        if !self.todos_dir.exists() {
            return Ok(Vec::new());
        }

        let mut plans = Vec::new();

        for entry in std::fs::read_dir(&self.todos_dir)
            .with_context(|| format!("Failed to read todos dir: {}", self.todos_dir.display()))?
        {
            let entry = entry.context("Failed to read directory entry")?;
            if !entry.file_type()?.is_dir() {
                continue;
            }

            let name = entry.file_name().to_string_lossy().to_string();

            // Skip hidden directories (.git, .lock, etc.)
            if name.starts_with('.') {
                continue;
            }

            let metadata_path = entry.path().join(METADATA_FILE);
            if !metadata_path.exists() {
                tracing::warn!(
                    timestamp = %name,
                    "TODO directory has no metadata.toml, skipping"
                );
                continue;
            }

            match self.load_inner(&name) {
                Ok(plan) => plans.push(plan),
                Err(e) => {
                    tracing::warn!(
                        timestamp = %name,
                        error = %e,
                        "Failed to load TODO plan, skipping"
                    );
                }
            }
        }

        // Newest first (timestamps sort lexicographically)
        plans.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        Ok(plans)
    }

    /// Find TODO plans by branch name.
    pub fn find_by_branch(&self, branch: &str) -> Result<Vec<TodoPlan>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|p| p.metadata.branch.as_deref() == Some(branch))
            .collect())
    }

    /// Find TODO plans by status.
    pub fn find_by_status(&self, status: TodoStatus) -> Result<Vec<TodoPlan>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|p| p.metadata.status == status)
            .collect())
    }

    // -- Internal helpers --------------------------------------------------

    fn create_inner(&self, title: &str, branch: Option<&str>) -> Result<TodoPlan> {
        let base_timestamp = Utc::now().format("%Y%m%dT%H%M%S").to_string();

        // Detect collision (multiple creates within the same second) and append suffix
        let mut timestamp = base_timestamp.clone();
        let mut suffix = 1u32;
        while self.todos_dir.join(&timestamp).exists() {
            timestamp = format!("{base_timestamp}-{suffix}");
            suffix += 1;
        }

        let plan_dir = self.todos_dir.join(&timestamp);

        std::fs::create_dir_all(&plan_dir)
            .with_context(|| format!("Failed to create TODO dir: {}", plan_dir.display()))?;

        let now = Utc::now();
        let metadata = TodoMetadata {
            branch: branch.map(|s| s.to_string()),
            status: TodoStatus::Draft,
            title: title.to_string(),
            sessions: Vec::new(),
            created_at: now,
            updated_at: now,
        };

        let plan = TodoPlan {
            timestamp: timestamp.clone(),
            todo_dir: plan_dir,
            metadata,
        };

        self.write_metadata(&plan)?;

        let initial_content = format!("# TODO: {title}\n\n## Goal\n\n## Tasks\n\n- [ ] \n");
        if let Err(e) = atomic_write(&plan.todo_md_path(), initial_content.as_bytes()) {
            // Rollback: remove the partially-created plan directory
            let _ = std::fs::remove_dir_all(&plan.todo_dir);
            return Err(e.context("Failed to write TODO.md (rolled back plan directory)"));
        }

        Ok(plan)
    }

    fn load_inner(&self, timestamp: &str) -> Result<TodoPlan> {
        validate_timestamp(timestamp)?;

        let plan_dir = self.todos_dir.join(timestamp);
        let metadata_path = plan_dir.join(METADATA_FILE);

        if !metadata_path.exists() {
            anyhow::bail!("TODO plan '{timestamp}' not found");
        }

        let content = std::fs::read_to_string(&metadata_path)
            .with_context(|| format!("Failed to read metadata: {}", metadata_path.display()))?;
        let metadata: TodoMetadata = toml::from_str(&content)
            .with_context(|| format!("Failed to parse metadata: {}", metadata_path.display()))?;

        Ok(TodoPlan {
            timestamp: timestamp.to_string(),
            todo_dir: plan_dir,
            metadata,
        })
    }

    fn write_metadata(&self, plan: &TodoPlan) -> Result<()> {
        let content =
            toml::to_string_pretty(&plan.metadata).context("Failed to serialize metadata")?;
        atomic_write(&plan.metadata_path(), content.as_bytes())
    }

    /// Acquire a write lock on the todos directory, execute `f`, then release.
    fn with_write_lock<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        std::fs::create_dir_all(&self.todos_dir).with_context(|| {
            format!(
                "Failed to create todos directory: {}",
                self.todos_dir.display()
            )
        })?;

        let lock_path = self.todos_dir.join(LOCK_FILE);
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("Failed to open lock file: {}", lock_path.display()))?;
        let mut lock = fd_lock::RwLock::new(lock_file);
        let _guard = lock
            .write()
            .map_err(|e| anyhow::anyhow!("Failed to acquire todo write lock: {e}"))?;

        f()
    }
}

/// Write data to a file atomically using temp-file + rename.
fn atomic_write(target: &Path, data: &[u8]) -> Result<()> {
    let parent = target.parent().context("Target path has no parent")?;

    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("Failed to create temp file in {}", parent.display()))?;

    std::io::Write::write_all(&mut tmp, data).context("Failed to write temp file")?;

    tmp.persist(target)
        .with_context(|| format!("Failed to persist to {}", target.display()))?;

    Ok(())
}

pub mod git;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_todo_status_roundtrip() {
        for status in [
            TodoStatus::Draft,
            TodoStatus::Debating,
            TodoStatus::Approved,
            TodoStatus::Implementing,
            TodoStatus::Done,
        ] {
            let s = status.to_string();
            let parsed: TodoStatus = s.parse().unwrap();
            assert_eq!(parsed, status);
        }
    }

    #[test]
    fn test_todo_status_invalid() {
        let result: Result<TodoStatus> = "invalid".parse();
        assert!(result.is_err());
    }

    #[test]
    fn test_create_plan() {
        let dir = tempdir().unwrap();
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

        let plan = manager.create("Test Plan", Some("feat/test")).unwrap();

        assert_eq!(plan.metadata.title, "Test Plan");
        assert_eq!(plan.metadata.branch.as_deref(), Some("feat/test"));
        assert_eq!(plan.metadata.status, TodoStatus::Draft);
        assert!(plan.metadata.sessions.is_empty());
        assert!(plan.todo_dir.exists());
        assert!(plan.metadata_path().exists());
        assert!(plan.todo_md_path().exists());
    }

    #[test]
    fn test_create_plan_no_branch() {
        let dir = tempdir().unwrap();
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

        let plan = manager.create("No Branch", None).unwrap();
        assert!(plan.metadata.branch.is_none());
    }

    #[test]
    fn test_load_plan() {
        let dir = tempdir().unwrap();
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

        let created = manager.create("Load Test", None).unwrap();
        let loaded = manager.load(&created.timestamp).unwrap();

        assert_eq!(loaded.metadata.title, "Load Test");
        assert_eq!(loaded.metadata.status, TodoStatus::Draft);
        assert_eq!(loaded.timestamp, created.timestamp);
    }

    #[test]
    fn test_load_nonexistent() {
        let dir = tempdir().unwrap();
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

        let result = manager.load("99991231T235959");
        assert!(result.is_err());
    }

    #[test]
    fn test_list_empty() {
        let dir = tempdir().unwrap();
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

        let plans = manager.list().unwrap();
        assert!(plans.is_empty());
    }

    #[test]
    fn test_list_nonexistent_dir() {
        let manager = TodoManager::with_base_dir(PathBuf::from("/nonexistent/todos"));
        let plans = manager.list().unwrap();
        assert!(plans.is_empty());
    }

    #[test]
    fn test_list_multiple() {
        let dir = tempdir().unwrap();
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

        manager.create("Plan A", None).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        manager.create("Plan B", None).unwrap();

        let plans = manager.list().unwrap();
        assert_eq!(plans.len(), 2);
        // Sorted newest first
        assert_eq!(plans[0].metadata.title, "Plan B");
        assert_eq!(plans[1].metadata.title, "Plan A");
    }

    #[test]
    fn test_list_skips_hidden_dirs() {
        let dir = tempdir().unwrap();
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

        manager.create("Visible", None).unwrap();
        // Simulate .git directory
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();

        let plans = manager.list().unwrap();
        assert_eq!(plans.len(), 1);
    }

    #[test]
    fn test_find_by_branch() {
        let dir = tempdir().unwrap();
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

        manager.create("Plan A", Some("feat/alpha")).unwrap();
        manager.create("Plan B", Some("feat/beta")).unwrap();
        manager.create("Plan C", Some("feat/alpha")).unwrap();

        let found = manager.find_by_branch("feat/alpha").unwrap();
        assert_eq!(found.len(), 2);
        assert!(
            found
                .iter()
                .all(|p| p.metadata.branch.as_deref() == Some("feat/alpha"))
        );
    }

    #[test]
    fn test_find_by_branch_none() {
        let dir = tempdir().unwrap();
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

        manager.create("Plan", Some("feat/other")).unwrap();

        let found = manager.find_by_branch("feat/missing").unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn test_find_by_status() {
        let dir = tempdir().unwrap();
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

        let plan = manager.create("Plan", None).unwrap();
        manager
            .update_status(&plan.timestamp, TodoStatus::Approved)
            .unwrap();
        manager.create("Draft Plan", None).unwrap();

        let approved = manager.find_by_status(TodoStatus::Approved).unwrap();
        assert_eq!(approved.len(), 1);
        assert_eq!(approved[0].metadata.title, "Plan");
    }

    #[test]
    fn test_update_status() {
        let dir = tempdir().unwrap();
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

        let plan = manager.create("Test", None).unwrap();
        assert_eq!(plan.metadata.status, TodoStatus::Draft);

        let updated = manager
            .update_status(&plan.timestamp, TodoStatus::Implementing)
            .unwrap();
        assert_eq!(updated.metadata.status, TodoStatus::Implementing);

        // Verify persisted
        let reloaded = manager.load(&plan.timestamp).unwrap();
        assert_eq!(reloaded.metadata.status, TodoStatus::Implementing);
    }

    #[test]
    fn test_update_status_nonexistent() {
        let dir = tempdir().unwrap();
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

        let result = manager.update_status("99991231T235959", TodoStatus::Done);
        assert!(result.is_err());
    }

    #[test]
    fn test_link_session() {
        let dir = tempdir().unwrap();
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

        let plan = manager.create("Test", None).unwrap();

        manager.link_session(&plan.timestamp, "01ABCDEF").unwrap();
        manager.link_session(&plan.timestamp, "01GHIJKL").unwrap();
        // Duplicate should be idempotent
        manager.link_session(&plan.timestamp, "01ABCDEF").unwrap();

        let reloaded = manager.load(&plan.timestamp).unwrap();
        assert_eq!(reloaded.metadata.sessions.len(), 2);
        assert!(reloaded.metadata.sessions.contains(&"01ABCDEF".to_string()));
        assert!(reloaded.metadata.sessions.contains(&"01GHIJKL".to_string()));
    }

    #[test]
    fn test_write_todo_md() {
        let dir = tempdir().unwrap();
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

        let plan = manager.create("Test", None).unwrap();

        let new_content = "# Updated\n\nNew content here.\n";
        manager.write_todo_md(&plan.timestamp, new_content).unwrap();

        let read_back = std::fs::read_to_string(plan.todo_md_path()).unwrap();
        assert_eq!(read_back, new_content);
    }

    #[test]
    fn test_metadata_serialization() {
        let metadata = TodoMetadata {
            branch: Some("feat/test".to_string()),
            status: TodoStatus::Draft,
            title: "Test".to_string(),
            sessions: vec!["01ABC".to_string()],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let toml_str = toml::to_string_pretty(&metadata).unwrap();
        let deserialized: TodoMetadata = toml::from_str(&toml_str).unwrap();

        assert_eq!(deserialized.title, "Test");
        assert_eq!(deserialized.status, TodoStatus::Draft);
        assert_eq!(deserialized.branch.as_deref(), Some("feat/test"));
        assert_eq!(deserialized.sessions, vec!["01ABC"]);
    }

    #[test]
    fn test_load_path_traversal_dotdot() {
        let dir = tempdir().unwrap();
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

        let result = manager.load("../etc/passwd");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("path traversal"));
    }

    #[test]
    fn test_load_path_traversal_absolute() {
        let dir = tempdir().unwrap();
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

        let result = manager.load("/tmp/evil");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Invalid timestamp"),
            "absolute path should be rejected"
        );
    }

    #[test]
    fn test_load_path_traversal_slash() {
        let dir = tempdir().unwrap();
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

        let result = manager.load("a/b");
        assert!(result.is_err());
    }

    #[test]
    fn test_load_empty_timestamp() {
        let dir = tempdir().unwrap();
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

        let result = manager.load("");
        assert!(result.is_err());
    }

    #[test]
    fn test_write_todo_md_updates_updated_at() {
        let dir = tempdir().unwrap();
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

        let plan = manager.create("Test", None).unwrap();
        let original_updated_at = plan.metadata.updated_at;

        // Small delay to ensure timestamp difference
        std::thread::sleep(std::time::Duration::from_millis(10));

        manager
            .write_todo_md(&plan.timestamp, "# Updated content\n")
            .unwrap();

        let reloaded = manager.load(&plan.timestamp).unwrap();
        assert!(reloaded.metadata.updated_at > original_updated_at);
    }

    #[test]
    fn test_latest() {
        let dir = tempdir().unwrap();
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

        manager.create("Plan A", None).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        let plan_b = manager.create("Plan B", None).unwrap();

        let latest = manager.latest().unwrap();
        assert_eq!(latest.metadata.title, "Plan B");
        assert_eq!(latest.timestamp, plan_b.timestamp);
    }

    #[test]
    fn test_latest_empty() {
        let dir = tempdir().unwrap();
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

        let result = manager.latest();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No TODO plans"));
    }

    #[test]
    fn test_status_lifecycle() {
        let dir = tempdir().unwrap();
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());

        let plan = manager.create("Lifecycle", None).unwrap();
        assert_eq!(plan.metadata.status, TodoStatus::Draft);

        let plan = manager
            .update_status(&plan.timestamp, TodoStatus::Debating)
            .unwrap();
        assert_eq!(plan.metadata.status, TodoStatus::Debating);

        let plan = manager
            .update_status(&plan.timestamp, TodoStatus::Approved)
            .unwrap();
        assert_eq!(plan.metadata.status, TodoStatus::Approved);

        let plan = manager
            .update_status(&plan.timestamp, TodoStatus::Implementing)
            .unwrap();
        assert_eq!(plan.metadata.status, TodoStatus::Implementing);

        let plan = manager
            .update_status(&plan.timestamp, TodoStatus::Done)
            .unwrap();
        assert_eq!(plan.metadata.status, TodoStatus::Done);
    }

    // Extended tests in lib_ext_tests.rs
    include!("lib_ext_tests.rs");
}
