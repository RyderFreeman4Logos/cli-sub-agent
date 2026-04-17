//! TODO plan management for CSA projects.
//!
//! Each project has a todos directory at `~/.local/state/cli-sub-agent/{project_path}/todos/`,
//! organized as a single git repository tracking all plans as timestamped subdirectories.
//!
//! # Directory Layout
//!
//! ```text
//! ~/.local/state/cli-sub-agent/{project_path}/todos/
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
const SPEC_FILE: &str = "spec.toml";
const LOCK_FILE: &str = ".lock";

pub use reference::{ReferenceFile, ReferenceIndex, ReferenceSource};
pub use spec::{CriterionKind, CriterionStatus, SpecCriterion, SpecDocument};

pub mod reference;
mod spec;

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
    /// Language for TODO content (e.g., "Chinese (Simplified)", "English").
    /// Patterns use this to enforce language consistency in plan content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
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
    /// `~/.local/state/cli-sub-agent/{encoded_project_path}/todos/`
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
        self.create_with_language(title, branch, None)
    }

    /// Create a new TODO plan with an optional language tag.
    pub fn create_with_language(
        &self,
        title: &str,
        branch: Option<&str>,
        language: Option<&str>,
    ) -> Result<TodoPlan> {
        self.with_write_lock(|| self.create_inner(title, branch, language))
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

    /// Update the title of a TODO plan.
    pub fn update_title(&self, timestamp: &str, title: &str) -> Result<TodoPlan> {
        self.with_write_lock(|| {
            let mut plan = self.load_inner(timestamp)?;
            plan.metadata.title = title.to_string();
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

    /// Return the path to spec.toml for the given plan timestamp.
    pub fn spec_path(&self, timestamp: &str) -> PathBuf {
        self.todos_dir.join(timestamp).join(SPEC_FILE)
    }

    /// Load spec.toml for a plan when it exists.
    pub fn load_spec(&self, timestamp: &str) -> Result<Option<SpecDocument>> {
        validate_timestamp(timestamp)?;

        let spec_path = self.spec_path(timestamp);
        if !spec_path.exists() {
            return Ok(None);
        }

        let content = std::fs::read_to_string(&spec_path)
            .with_context(|| format!("Failed to read spec: {}", spec_path.display()))?;
        let spec: SpecDocument = toml::from_str(&content)
            .with_context(|| format!("Failed to parse spec: {}", spec_path.display()))?;
        Ok(Some(spec))
    }

    /// Save spec.toml for an existing TODO plan.
    pub fn save_spec(&self, timestamp: &str, spec: &SpecDocument) -> Result<()> {
        self.with_write_lock(|| {
            validate_timestamp(timestamp)?;

            let metadata_path = self.todos_dir.join(timestamp).join(METADATA_FILE);
            if !metadata_path.exists() {
                anyhow::bail!("TODO plan '{timestamp}' not found");
            }

            let content = toml::to_string_pretty(spec).context("Failed to serialize spec")?;
            atomic_write(&self.spec_path(timestamp), content.as_bytes())
        })
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
        plans.sort_by_key(|plan| std::cmp::Reverse(plan.timestamp.clone()));

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

    fn create_inner(
        &self,
        title: &str,
        branch: Option<&str>,
        language: Option<&str>,
    ) -> Result<TodoPlan> {
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
            language: language.map(|s| s.to_string()),
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

pub mod dag;
pub mod git;
pub mod redact;
pub mod token_estimate;
pub mod xurl_integration;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
