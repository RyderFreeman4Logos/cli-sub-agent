//! Persistent storage for compressed tool outputs.
//!
//! When tool output compression is enabled, large outputs are replaced
//! in-context with a file path reference. This module handles persisting
//! the original content and providing retrieval.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Manifest entry describing a single compressed tool output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// Tool call index within the session.
    pub index: u32,
    /// Original byte count before compression.
    pub original_bytes: u64,
    /// Path to the stored raw content (relative to session dir).
    pub path: String,
}

/// TOML-serializable manifest for all compressed outputs in a session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub entries: Vec<ManifestEntry>,
}

/// Manages storage of compressed tool outputs within a session directory.
pub struct ToolOutputStore {
    base_dir: PathBuf,
}

impl ToolOutputStore {
    /// Create a new store rooted at `{session_dir}/tool_outputs/`.
    ///
    /// Creates the directory if it does not exist.
    pub fn new(session_dir: &Path) -> Result<Self> {
        let base_dir = session_dir.join("tool_outputs");
        fs::create_dir_all(&base_dir).with_context(|| {
            format!("failed to create tool_outputs dir: {}", base_dir.display())
        })?;
        Ok(Self { base_dir })
    }

    /// Store raw tool output content, returning the file path.
    pub fn store(&self, index: u32, content: &[u8]) -> Result<PathBuf> {
        let filename = format!("{index}.raw");
        let path = self.base_dir.join(&filename);
        fs::write(&path, content)
            .with_context(|| format!("failed to write tool output: {}", path.display()))?;
        Ok(path)
    }

    /// Load previously stored tool output content.
    pub fn load(&self, index: u32) -> Result<Vec<u8>> {
        let filename = format!("{index}.raw");
        let path = self.base_dir.join(&filename);
        fs::read(&path).with_context(|| format!("failed to read tool output: {}", path.display()))
    }

    /// Path to the manifest file.
    pub fn manifest_path(&self) -> PathBuf {
        self.base_dir.join("manifest.toml")
    }

    /// Append an entry to the manifest file.
    ///
    /// Creates the manifest if it does not exist; appends otherwise.
    pub fn append_manifest(&self, index: u32, original_bytes: u64) -> Result<()> {
        let manifest_path = self.manifest_path();
        let mut manifest = if manifest_path.exists() {
            let content =
                fs::read_to_string(&manifest_path).with_context(|| "failed to read manifest")?;
            toml::from_str::<Manifest>(&content).with_context(|| "failed to parse manifest")?
        } else {
            Manifest::default()
        };

        // Use relative path from base_dir's parent (session dir).
        let relative = format!("tool_outputs/{index}.raw");
        manifest.entries.push(ManifestEntry {
            index,
            original_bytes,
            path: relative,
        });

        let serialized =
            toml::to_string_pretty(&manifest).with_context(|| "failed to serialize manifest")?;
        fs::write(&manifest_path, serialized).with_context(|| "failed to write manifest")?;
        Ok(())
    }

    /// Read the full manifest, returning an empty manifest if the file does not exist.
    pub fn read_manifest(&self) -> Result<Manifest> {
        let manifest_path = self.manifest_path();
        if !manifest_path.exists() {
            return Ok(Manifest::default());
        }
        let content =
            fs::read_to_string(&manifest_path).with_context(|| "failed to read manifest")?;
        toml::from_str::<Manifest>(&content).with_context(|| "failed to parse manifest")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_store_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let store = ToolOutputStore::new(tmp.path()).unwrap();

        let content = b"hello world, this is a large tool output";
        let path = store.store(0, content).unwrap();
        assert!(path.exists());

        let loaded = store.load(0).unwrap();
        assert_eq!(loaded, content);
    }

    #[test]
    fn test_store_multiple_indices() {
        let tmp = TempDir::new().unwrap();
        let store = ToolOutputStore::new(tmp.path()).unwrap();

        store.store(0, b"first").unwrap();
        store.store(1, b"second").unwrap();
        store.store(5, b"fifth").unwrap();

        assert_eq!(store.load(0).unwrap(), b"first");
        assert_eq!(store.load(1).unwrap(), b"second");
        assert_eq!(store.load(5).unwrap(), b"fifth");
    }

    #[test]
    fn test_load_nonexistent_returns_error() {
        let tmp = TempDir::new().unwrap();
        let store = ToolOutputStore::new(tmp.path()).unwrap();
        assert!(store.load(99).is_err());
    }

    #[test]
    fn test_manifest_append_and_read() {
        let tmp = TempDir::new().unwrap();
        let store = ToolOutputStore::new(tmp.path()).unwrap();

        store.store(0, b"content").unwrap();
        store.append_manifest(0, 7).unwrap();

        store.store(1, b"more content").unwrap();
        store.append_manifest(1, 12).unwrap();

        let manifest = store.read_manifest().unwrap();
        assert_eq!(manifest.entries.len(), 2);
        assert_eq!(manifest.entries[0].index, 0);
        assert_eq!(manifest.entries[0].original_bytes, 7);
        assert_eq!(manifest.entries[1].index, 1);
        assert_eq!(manifest.entries[1].original_bytes, 12);
    }

    #[test]
    fn test_read_empty_manifest() {
        let tmp = TempDir::new().unwrap();
        let store = ToolOutputStore::new(tmp.path()).unwrap();

        let manifest = store.read_manifest().unwrap();
        assert!(manifest.entries.is_empty());
    }

    #[test]
    fn test_manifest_path() {
        let tmp = TempDir::new().unwrap();
        let store = ToolOutputStore::new(tmp.path()).unwrap();
        assert_eq!(
            store.manifest_path(),
            tmp.path().join("tool_outputs").join("manifest.toml")
        );
    }
}
