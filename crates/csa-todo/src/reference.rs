//! Reference file management for TODO plans.
//!
//! Each TODO plan can have an optional `references/` subdirectory containing
//! supporting documents (recon summaries, transcript excerpts, context files).
//! Files are indexed via `references/index.toml` for progressive disclosure.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use crate::{TodoManager, TodoPlan, atomic_write, token_estimate};

const REFERENCES_DIR: &str = "references";
const INDEX_FILE: &str = "index.toml";

/// Regex for valid reference filenames: alphanumeric, underscore, hyphen, dot,
/// ending with `.md`.
static FILENAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9_.-]+\.md$").expect("valid regex"));

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// How a reference file was produced.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReferenceSource {
    /// Manually added by the user or agent.
    Manual,
    /// Extracted from a tool transcript.
    Transcript { tool: String, session: String },
    /// Produced by a reconnaissance session.
    ReconSession { session: String },
}

/// A single reference file within a plan's `references/` directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceFile {
    /// Filename (e.g., `recon-summary.md`).
    pub name: String,
    /// Absolute path to the file on disk (not serialized to index).
    #[serde(skip)]
    pub path: PathBuf,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Estimated token count (chars / 3 heuristic).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_estimate: Option<usize>,
    /// How this reference was produced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<ReferenceSource>,
    /// When this reference was added.
    pub created_at: DateTime<Utc>,
}

/// Index of all reference files for a plan.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReferenceIndex {
    /// Individual reference file entries.
    pub files: Vec<ReferenceFile>,
    /// Sum of all token estimates (when computed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<usize>,
}

// ---------------------------------------------------------------------------
// TodoPlan extension
// ---------------------------------------------------------------------------

impl TodoPlan {
    /// Path to the `references/` subdirectory for this plan.
    pub fn references_dir(&self) -> PathBuf {
        self.todo_dir.join(REFERENCES_DIR)
    }
}

// ---------------------------------------------------------------------------
// Filename validation
// ---------------------------------------------------------------------------

/// Validate a reference filename.
///
/// Accepts: `recon-summary.md`, `arch_notes.md`, `v2.design.md`
/// Rejects: `../evil.md`, `foo/bar.md`, `test.txt`, empty string
fn validate_reference_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("Reference filename must not be empty");
    }
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        anyhow::bail!("Invalid reference filename (path traversal detected): '{name}'");
    }
    if !FILENAME_RE.is_match(name) {
        anyhow::bail!(
            "Invalid reference filename: '{name}'. \
             Must match [a-zA-Z0-9_.-]+.md"
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Token estimation (inline heuristic for content already in memory)
// ---------------------------------------------------------------------------

/// Estimate token count from content already loaded in memory (chars / 3 heuristic).
fn estimate_tokens_inline(content: &str) -> usize {
    token_estimate::estimate_tokens_heuristic(content)
}

// ---------------------------------------------------------------------------
// TodoManager reference methods
// ---------------------------------------------------------------------------

impl TodoManager {
    /// Lazily create the `references/` directory for a plan, returning its path.
    pub fn ensure_references_dir(&self, plan: &TodoPlan) -> Result<PathBuf> {
        let refs_dir = plan.references_dir();
        if !refs_dir.exists() {
            std::fs::create_dir_all(&refs_dir).with_context(|| {
                format!("Failed to create references dir: {}", refs_dir.display())
            })?;
        }
        Ok(refs_dir)
    }

    /// Write a reference file to a plan's `references/` directory.
    ///
    /// Validates the filename, writes the content atomically, and updates
    /// `references/index.toml`.
    pub fn write_reference(
        &self,
        plan: &TodoPlan,
        name: &str,
        content: &str,
        source: Option<ReferenceSource>,
    ) -> Result<()> {
        validate_reference_name(name)?;

        self.with_write_lock(|| {
            let refs_dir = self.ensure_references_dir(plan)?;
            let file_path = refs_dir.join(name);

            // Write file content atomically
            atomic_write(&file_path, content.as_bytes())
                .with_context(|| format!("Failed to write reference: {name}"))?;

            // Update index
            let index_path = refs_dir.join(INDEX_FILE);
            let mut index = self.load_index_inner(&index_path)?;

            // Remove existing entry for this name (upsert semantics)
            index.files.retain(|f| f.name != name);

            let size_bytes = content.len() as u64;
            let token_estimate = estimate_tokens_inline(content);

            index.files.push(ReferenceFile {
                name: name.to_string(),
                path: PathBuf::new(), // skipped in serialization
                size_bytes,
                token_estimate: Some(token_estimate),
                source,
                created_at: Utc::now(),
            });

            // Recompute total tokens
            index.total_tokens = Some(index.files.iter().filter_map(|f| f.token_estimate).sum());

            let index_content =
                toml::to_string_pretty(&index).context("Failed to serialize reference index")?;
            atomic_write(&index_path, index_content.as_bytes())
                .context("Failed to write reference index")?;

            Ok(())
        })
    }

    /// Read a reference file from a plan's `references/` directory.
    ///
    /// If `max_tokens` is provided and the content exceeds that estimate,
    /// returns an error with the actual token estimate.
    pub fn read_reference(
        &self,
        plan: &TodoPlan,
        name: &str,
        max_tokens: Option<usize>,
    ) -> Result<String> {
        validate_reference_name(name)?;

        let file_path = plan.references_dir().join(name);

        if let Some(max) = max_tokens {
            let estimated = token_estimate::estimate_tokens(&file_path)
                .with_context(|| format!("Failed to estimate tokens for reference: {name}"))?;
            if estimated > max {
                anyhow::bail!(
                    "Reference '{name}' exceeds token budget: \
                     ~{estimated} tokens (max: {max}). \
                     Use `csa todo ref show --max-tokens {estimated}` to override, \
                     or load a summary instead."
                );
            }
        }

        let content = std::fs::read_to_string(&file_path)
            .with_context(|| format!("Failed to read reference: {name}"))?;

        Ok(content)
    }

    /// List all reference files for a plan.
    ///
    /// Returns an empty `ReferenceIndex` if the `references/` directory does
    /// not exist. When `with_tokens` is true, token estimates are computed
    /// for each file.
    pub fn list_references(&self, plan: &TodoPlan, with_tokens: bool) -> Result<ReferenceIndex> {
        let refs_dir = plan.references_dir();
        if !refs_dir.exists() {
            return Ok(ReferenceIndex::default());
        }

        let index_path = refs_dir.join(INDEX_FILE);
        let mut index = if index_path.exists() {
            self.load_index_inner(&index_path)?
        } else {
            // No index file — scan directory
            self.scan_references_dir(&refs_dir)?
        };

        // Populate path field (skipped in serialization) and optionally compute tokens
        let mut index_dirty = false;
        for entry in &mut index.files {
            entry.path = refs_dir.join(&entry.name);

            if with_tokens && entry.token_estimate.is_none() {
                if let Ok(count) = token_estimate::estimate_tokens(&entry.path) {
                    entry.token_estimate = Some(count);
                    index_dirty = true;
                }
            }
        }

        if with_tokens {
            index.total_tokens = Some(index.files.iter().filter_map(|f| f.token_estimate).sum());

            // Persist newly computed estimates back to index.toml
            if index_dirty {
                let index_path = refs_dir.join(INDEX_FILE);
                if let Ok(serialized) = toml::to_string_pretty(&index) {
                    let _ = atomic_write(&index_path, serialized.as_bytes());
                }
            }
        }

        Ok(index)
    }

    // -- Internal helpers --------------------------------------------------

    /// Load an existing index.toml, or return an empty index.
    fn load_index_inner(&self, index_path: &Path) -> Result<ReferenceIndex> {
        if !index_path.exists() {
            return Ok(ReferenceIndex::default());
        }
        let content = std::fs::read_to_string(index_path)
            .with_context(|| format!("Failed to read index: {}", index_path.display()))?;
        let index: ReferenceIndex = toml::from_str(&content)
            .with_context(|| format!("Failed to parse index: {}", index_path.display()))?;
        Ok(index)
    }

    /// Scan a references directory for `.md` files (fallback when no index exists).
    fn scan_references_dir(&self, refs_dir: &Path) -> Result<ReferenceIndex> {
        let mut files = Vec::new();

        for entry in std::fs::read_dir(refs_dir)
            .with_context(|| format!("Failed to read references dir: {}", refs_dir.display()))?
        {
            let entry = entry.context("Failed to read directory entry")?;
            let name = entry.file_name().to_string_lossy().to_string();

            // Skip index.toml and non-.md files
            if name == INDEX_FILE || !name.ends_with(".md") {
                continue;
            }

            let metadata = entry.metadata().context("Failed to read file metadata")?;
            if !metadata.is_file() {
                continue;
            }

            files.push(ReferenceFile {
                name,
                path: entry.path(),
                size_bytes: metadata.len(),
                token_estimate: None,
                source: None,
                created_at: DateTime::from(
                    metadata.created().unwrap_or(std::time::SystemTime::now()),
                ),
            });
        }

        files.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(ReferenceIndex {
            files,
            total_tokens: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Helper: create a TodoManager and a plan in a temp directory.
    fn setup() -> (tempfile::TempDir, TodoManager, TodoPlan) {
        let dir = tempdir().unwrap();
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());
        let plan = manager.create("Test Plan", None).unwrap();
        (dir, manager, plan)
    }

    // -- Filename validation -----------------------------------------------

    #[test]
    fn test_validate_reference_name_valid() {
        assert!(validate_reference_name("recon-summary.md").is_ok());
        assert!(validate_reference_name("arch_notes.md").is_ok());
        assert!(validate_reference_name("v2.design.md").is_ok());
        assert!(validate_reference_name("A.md").is_ok());
        assert!(validate_reference_name("123.md").is_ok());
    }

    #[test]
    fn test_validate_reference_name_rejects_path_traversal() {
        let result = validate_reference_name("../evil.md");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("path traversal"));
    }

    #[test]
    fn test_validate_reference_name_rejects_slash() {
        assert!(validate_reference_name("foo/bar.md").is_err());
    }

    #[test]
    fn test_validate_reference_name_rejects_backslash() {
        assert!(validate_reference_name("foo\\bar.md").is_err());
    }

    #[test]
    fn test_validate_reference_name_rejects_wrong_extension() {
        assert!(validate_reference_name("test.txt").is_err());
        assert!(validate_reference_name("data.toml").is_err());
        assert!(validate_reference_name("script.rs").is_err());
    }

    #[test]
    fn test_validate_reference_name_rejects_absolute_path() {
        assert!(validate_reference_name("/etc/passwd").is_err());
    }

    #[test]
    fn test_validate_reference_name_rejects_empty() {
        assert!(validate_reference_name("").is_err());
    }

    #[test]
    fn test_validate_reference_name_rejects_no_extension() {
        assert!(validate_reference_name("noext").is_err());
    }

    // -- write_reference + read_reference roundtrip ------------------------

    #[test]
    fn test_write_and_read_reference_roundtrip() {
        let (_dir, manager, plan) = setup();

        let content = "# Recon Summary\n\nFound 3 modules.\n";
        manager
            .write_reference(&plan, "recon-summary.md", content, None)
            .unwrap();

        let read_back = manager
            .read_reference(&plan, "recon-summary.md", None)
            .unwrap();
        assert_eq!(read_back, content);
    }

    #[test]
    fn test_write_reference_with_source() {
        let (_dir, manager, plan) = setup();

        let source = Some(ReferenceSource::Transcript {
            tool: "gemini-cli".to_string(),
            session: "01ABC".to_string(),
        });

        manager
            .write_reference(&plan, "transcript.md", "# Transcript\n", source)
            .unwrap();

        // Verify index records the source
        let index = manager.list_references(&plan, false).unwrap();
        assert_eq!(index.files.len(), 1);
        assert!(index.files[0].source.is_some());
        match &index.files[0].source {
            Some(ReferenceSource::Transcript { tool, session }) => {
                assert_eq!(tool, "gemini-cli");
                assert_eq!(session, "01ABC");
            }
            other => panic!("Expected Transcript source, got: {other:?}"),
        }
    }

    #[test]
    fn test_write_reference_upsert() {
        let (_dir, manager, plan) = setup();

        manager
            .write_reference(&plan, "notes.md", "v1", None)
            .unwrap();
        manager
            .write_reference(&plan, "notes.md", "v2", None)
            .unwrap();

        let content = manager.read_reference(&plan, "notes.md", None).unwrap();
        assert_eq!(content, "v2");

        let index = manager.list_references(&plan, false).unwrap();
        assert_eq!(index.files.len(), 1, "upsert should not duplicate entries");
    }

    #[test]
    fn test_write_reference_rejects_invalid_name() {
        let (_dir, manager, plan) = setup();

        assert!(
            manager
                .write_reference(&plan, "../evil.md", "hack", None)
                .is_err()
        );
        assert!(
            manager
                .write_reference(&plan, "foo/bar.md", "hack", None)
                .is_err()
        );
        assert!(
            manager
                .write_reference(&plan, "test.txt", "hack", None)
                .is_err()
        );
    }

    // -- read_reference with max_tokens ------------------------------------

    #[test]
    fn test_read_reference_within_token_budget() {
        let (_dir, manager, plan) = setup();

        let content = "x".repeat(300); // ~100 tokens
        manager
            .write_reference(&plan, "small.md", &content, None)
            .unwrap();

        let result = manager.read_reference(&plan, "small.md", Some(200));
        assert!(result.is_ok());
    }

    #[test]
    fn test_read_reference_exceeds_token_budget() {
        let (_dir, manager, plan) = setup();

        let content = "x".repeat(3000); // ~1000 tokens
        manager
            .write_reference(&plan, "large.md", &content, None)
            .unwrap();

        let result = manager.read_reference(&plan, "large.md", Some(500));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("exceeds token budget"), "got: {err}");
        assert!(
            err.contains("1000"),
            "should report actual estimate, got: {err}"
        );
    }

    #[test]
    fn test_read_reference_nonexistent() {
        let (_dir, manager, plan) = setup();

        let result = manager.read_reference(&plan, "missing.md", None);
        assert!(result.is_err());
    }

    // -- list_references ---------------------------------------------------

    #[test]
    fn test_list_references_empty_no_dir() {
        let (_dir, manager, plan) = setup();

        let index = manager.list_references(&plan, false).unwrap();
        assert!(index.files.is_empty());
        assert!(index.total_tokens.is_none());
    }

    #[test]
    fn test_list_references_populated() {
        let (_dir, manager, plan) = setup();

        manager
            .write_reference(&plan, "a.md", "alpha", None)
            .unwrap();
        manager
            .write_reference(&plan, "b.md", "beta content here", None)
            .unwrap();

        let index = manager.list_references(&plan, false).unwrap();
        assert_eq!(index.files.len(), 2);

        let names: Vec<&str> = index.files.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"a.md"));
        assert!(names.contains(&"b.md"));
    }

    #[test]
    fn test_list_references_with_tokens() {
        let (_dir, manager, plan) = setup();

        let content_a = "x".repeat(300); // ~100 tokens
        let content_b = "y".repeat(600); // ~200 tokens
        manager
            .write_reference(&plan, "a.md", &content_a, None)
            .unwrap();
        manager
            .write_reference(&plan, "b.md", &content_b, None)
            .unwrap();

        let index = manager.list_references(&plan, true).unwrap();
        assert_eq!(index.files.len(), 2);
        assert!(index.total_tokens.is_some());
        assert_eq!(index.total_tokens.unwrap(), 300); // 100 + 200
    }

    // -- index.toml persistence --------------------------------------------

    #[test]
    fn test_index_toml_created_on_write() {
        let (_dir, manager, plan) = setup();

        manager
            .write_reference(&plan, "doc.md", "content", None)
            .unwrap();

        let index_path = plan.references_dir().join(INDEX_FILE);
        assert!(index_path.exists(), "index.toml should be created");

        let content = std::fs::read_to_string(&index_path).unwrap();
        assert!(content.contains("doc.md"));
    }

    #[test]
    fn test_index_toml_updated_on_second_write() {
        let (_dir, manager, plan) = setup();

        manager
            .write_reference(&plan, "first.md", "1", None)
            .unwrap();
        manager
            .write_reference(&plan, "second.md", "22", None)
            .unwrap();

        let index_path = plan.references_dir().join(INDEX_FILE);
        let content = std::fs::read_to_string(&index_path).unwrap();
        assert!(content.contains("first.md"));
        assert!(content.contains("second.md"));
    }

    #[test]
    fn test_index_records_size_bytes() {
        let (_dir, manager, plan) = setup();

        let content = "hello world";
        manager
            .write_reference(&plan, "sized.md", content, None)
            .unwrap();

        let index = manager.list_references(&plan, false).unwrap();
        assert_eq!(index.files[0].size_bytes, content.len() as u64);
    }

    // -- ensure_references_dir ---------------------------------------------

    #[test]
    fn test_ensure_references_dir_creates_dir() {
        let (_dir, manager, plan) = setup();

        assert!(!plan.references_dir().exists());

        let path = manager.ensure_references_dir(&plan).unwrap();
        assert!(path.exists());
        assert!(path.is_dir());
        assert_eq!(path, plan.references_dir());
    }

    #[test]
    fn test_ensure_references_dir_idempotent() {
        let (_dir, manager, plan) = setup();

        let p1 = manager.ensure_references_dir(&plan).unwrap();
        let p2 = manager.ensure_references_dir(&plan).unwrap();
        assert_eq!(p1, p2);
    }

    // -- scan fallback (no index.toml) -------------------------------------

    #[test]
    fn test_list_references_scans_when_no_index() {
        let (_dir, _manager, plan) = setup();

        // Manually create references dir with files but no index.toml
        let refs_dir = plan.references_dir();
        std::fs::create_dir_all(&refs_dir).unwrap();
        std::fs::write(refs_dir.join("manual.md"), "manual content").unwrap();
        std::fs::write(refs_dir.join("other.md"), "other").unwrap();
        // Non-.md files should be ignored
        std::fs::write(refs_dir.join("data.json"), "{}").unwrap();

        let manager = TodoManager::with_base_dir(plan.todo_dir.parent().unwrap().to_path_buf());
        let index = manager.list_references(&plan, true).unwrap();

        assert_eq!(index.files.len(), 2);
        let names: Vec<&str> = index.files.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"manual.md"));
        assert!(names.contains(&"other.md"));
        assert!(!names.contains(&"data.json"));
    }

    // -- ReferenceSource serialization -------------------------------------

    #[test]
    fn test_reference_source_serde_roundtrip() {
        let sources = vec![
            ReferenceSource::Manual,
            ReferenceSource::Transcript {
                tool: "claude-code".to_string(),
                session: "01XYZ".to_string(),
            },
            ReferenceSource::ReconSession {
                session: "01ABC".to_string(),
            },
        ];

        for source in sources {
            let toml_str = toml::to_string(&source).unwrap();
            let parsed: ReferenceSource = toml::from_str(&toml_str).unwrap();
            // Verify tag-based serialization roundtrips
            let toml_str2 = toml::to_string(&parsed).unwrap();
            assert_eq!(toml_str, toml_str2);
        }
    }

    #[test]
    fn test_reference_index_serde_roundtrip() {
        let index = ReferenceIndex {
            files: vec![ReferenceFile {
                name: "test.md".to_string(),
                path: PathBuf::new(),
                size_bytes: 42,
                token_estimate: Some(14),
                source: Some(ReferenceSource::Manual),
                created_at: Utc::now(),
            }],
            total_tokens: Some(14),
        };

        let toml_str = toml::to_string_pretty(&index).unwrap();
        let parsed: ReferenceIndex = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.files.len(), 1);
        assert_eq!(parsed.files[0].name, "test.md");
        assert_eq!(parsed.total_tokens, Some(14));
    }

    // -- Token estimation integration -----------------------------------------

    #[test]
    fn test_list_references_with_tokens_populates_estimates() {
        let (_dir, _manager, plan) = setup();

        // Write a reference without pre-cached token estimate by
        // manually creating the file and a minimal index entry
        let refs_dir = plan.references_dir();
        std::fs::create_dir_all(&refs_dir).unwrap();
        let content = "a".repeat(600); // 600 chars -> ~200 tokens
        std::fs::write(refs_dir.join("uncached.md"), &content).unwrap();

        // Write index with no token_estimate
        let index = ReferenceIndex {
            files: vec![ReferenceFile {
                name: "uncached.md".to_string(),
                path: PathBuf::new(),
                size_bytes: 600,
                token_estimate: None, // not cached
                source: None,
                created_at: Utc::now(),
            }],
            total_tokens: None,
        };
        let index_str = toml::to_string_pretty(&index).unwrap();
        std::fs::write(refs_dir.join(INDEX_FILE), &index_str).unwrap();

        let manager = TodoManager::with_base_dir(plan.todo_dir.parent().unwrap().to_path_buf());
        let result = manager.list_references(&plan, true).unwrap();

        assert_eq!(result.files.len(), 1);
        assert!(
            result.files[0].token_estimate.is_some(),
            "Token estimate should be populated"
        );
        assert_eq!(result.files[0].token_estimate.unwrap(), 200);
        assert_eq!(result.total_tokens, Some(200));
    }

    #[test]
    fn test_list_references_cached_estimates_reused() {
        let (_dir, _manager, plan) = setup();

        // Write a reference with a known cached estimate in index
        let refs_dir = plan.references_dir();
        std::fs::create_dir_all(&refs_dir).unwrap();
        std::fs::write(refs_dir.join("cached.md"), "content").unwrap();

        let cached_estimate = 42; // intentionally wrong to prove cache is used
        let index = ReferenceIndex {
            files: vec![ReferenceFile {
                name: "cached.md".to_string(),
                path: PathBuf::new(),
                size_bytes: 7,
                token_estimate: Some(cached_estimate),
                source: None,
                created_at: Utc::now(),
            }],
            total_tokens: Some(cached_estimate),
        };
        let index_str = toml::to_string_pretty(&index).unwrap();
        std::fs::write(refs_dir.join(INDEX_FILE), &index_str).unwrap();

        let manager = TodoManager::with_base_dir(plan.todo_dir.parent().unwrap().to_path_buf());
        let result = manager.list_references(&plan, true).unwrap();

        // Should use cached value, not re-estimate
        assert_eq!(result.files[0].token_estimate, Some(cached_estimate));
        assert_eq!(result.total_tokens, Some(cached_estimate));
    }

    #[test]
    fn test_read_reference_max_tokens_error_includes_suggestion() {
        let (_dir, manager, plan) = setup();

        let content = "x".repeat(3000); // ~1000 tokens
        manager
            .write_reference(&plan, "big.md", &content, None)
            .unwrap();

        let result = manager.read_reference(&plan, "big.md", Some(500));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("csa todo ref show --max-tokens"),
            "Error should suggest override command, got: {err}"
        );
    }
}
