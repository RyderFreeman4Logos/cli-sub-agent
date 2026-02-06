use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Per-project usage statistics, stored in TOML.
/// Keeps last 20 records per tool for P95 estimation.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct UsageStats {
    #[serde(default)]
    history: HashMap<String, Vec<u64>>,
}

impl UsageStats {
    /// P95 estimate: take 95th percentile from sorted history.
    /// Returns None if no history exists for this tool.
    pub fn get_p95_estimate(&self, tool: &str) -> Option<u64> {
        let records = self.history.get(tool)?;
        if records.is_empty() {
            return None;
        }

        let mut sorted = records.clone();
        sorted.sort_unstable();

        let idx = ((sorted.len() as f64) * 0.95).ceil() as usize;
        let idx = idx.min(sorted.len()).saturating_sub(1);
        Some(sorted[idx])
    }

    /// Record a usage observation. Keeps last 20 entries.
    pub fn record(&mut self, tool: &str, usage_mb: u64) {
        let entry = self.history.entry(tool.to_string()).or_default();
        entry.push(usage_mb);
        if entry.len() > 20 {
            entry.remove(0); // Remove oldest
        }
    }

    /// Load from file. Returns default if file doesn't exist.
    pub fn load(stats_path: &Path) -> Result<Self> {
        if !stats_path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(stats_path)?;
        Ok(toml::from_str(&content)?)
    }

    /// Save to file. Creates parent directories if needed.
    pub fn save(&self, stats_path: &Path) -> Result<()> {
        if let Some(parent) = stats_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(stats_path, content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_p95_estimate_empty_history() {
        let stats = UsageStats::default();
        assert_eq!(stats.get_p95_estimate("tool1"), None);
    }

    #[test]
    fn test_p95_estimate_single_record() {
        let mut stats = UsageStats::default();
        stats.record("tool1", 100);
        assert_eq!(stats.get_p95_estimate("tool1"), Some(100));
    }

    #[test]
    fn test_p95_estimate_with_20_records() {
        let mut stats = UsageStats::default();
        // Add 20 records: 1, 2, 3, ..., 20
        for i in 1..=20 {
            stats.record("tool1", i);
        }
        // P95 of 20 items: ceil(20 * 0.95) = 19, so 19th item (0-indexed = 18) = 19
        assert_eq!(stats.get_p95_estimate("tool1"), Some(19));
    }

    #[test]
    fn test_record_keeps_max_20_entries() {
        let mut stats = UsageStats::default();
        // Add 25 records
        for i in 1..=25 {
            stats.record("tool1", i);
        }
        // Should keep last 20: 6, 7, ..., 25
        let records = stats.history.get("tool1").unwrap();
        assert_eq!(records.len(), 20);
        assert_eq!(records[0], 6); // Oldest remaining
        assert_eq!(records[19], 25); // Newest
    }

    #[test]
    fn test_record_removes_oldest() {
        let mut stats = UsageStats::default();
        // Add 20 records
        for i in 1..=20 {
            stats.record("tool1", i);
        }
        // Add one more
        stats.record("tool1", 999);
        let records = stats.history.get("tool1").unwrap();
        assert_eq!(records.len(), 20);
        assert_eq!(records[0], 2); // First record (1) was removed
        assert_eq!(records[19], 999); // New record at end
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let dir = tempdir().unwrap();
        let stats_path = dir.path().join("stats.toml");

        let mut stats = UsageStats::default();
        stats.record("tool1", 100);
        stats.record("tool1", 200);
        stats.record("tool2", 300);

        stats.save(&stats_path).unwrap();

        let loaded = UsageStats::load(&stats_path).unwrap();
        assert_eq!(loaded.get_p95_estimate("tool1"), Some(200));
        assert_eq!(loaded.get_p95_estimate("tool2"), Some(300));
    }

    #[test]
    fn test_default_has_empty_history() {
        let stats = UsageStats::default();
        assert!(stats.history.is_empty());
    }
}
