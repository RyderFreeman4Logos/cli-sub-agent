use std::collections::{HashMap, HashSet};

use anyhow::Result;
use chrono::Utc;
use ulid::Ulid;

use crate::llm_client::MemoryLlmClient;
use crate::{MemoryEntry, MemoryIndex, MemorySource, MemoryStore};

/// Plan for consolidation, returned by dry-run.
#[derive(Debug)]
pub struct ConsolidationPlan {
    /// ULIDs to mark with `valid_until`.
    pub entries_to_expire: Vec<String>,
    /// Groups of entries that can be merged into one consolidated entry.
    pub groups_to_merge: Vec<MergeGroup>,
    pub total_before: usize,
    pub total_after_estimate: usize,
}

#[derive(Debug)]
pub struct MergeGroup {
    pub source_ids: Vec<String>,
    pub merged_content_preview: String,
    pub full_summary: String,
    pub merged_tags: Vec<String>,
}

/// Generate a consolidation plan (dry-run).
pub async fn plan_consolidation(
    store: &MemoryStore,
    client: &dyn MemoryLlmClient,
) -> Result<ConsolidationPlan> {
    let entries = store.load_all()?;
    if entries.len() < 10 {
        return Ok(ConsolidationPlan {
            entries_to_expire: Vec::new(),
            groups_to_merge: Vec::new(),
            total_before: entries.len(),
            total_after_estimate: entries.len(),
        });
    }

    let mut by_project: HashMap<String, Vec<MemoryEntry>> = HashMap::new();
    for entry in entries {
        let project_key = entry
            .project
            .clone()
            .unwrap_or_else(|| "global".to_string());
        by_project.entry(project_key).or_default().push(entry);
    }

    let mut plan = ConsolidationPlan {
        entries_to_expire: Vec::new(),
        groups_to_merge: Vec::new(),
        total_before: by_project.values().map(Vec::len).sum(),
        total_after_estimate: by_project.values().map(Vec::len).sum(),
    };

    for project_entries in by_project.values() {
        if project_entries.len() < 5 {
            continue;
        }

        let full_summary = client.summarize(project_entries).await?;
        if full_summary.trim().is_empty() {
            continue;
        }

        let source_ids: Vec<String> = project_entries
            .iter()
            .map(|entry| entry.id.to_string())
            .collect();
        let preview_len = full_summary
            .char_indices()
            .nth(200)
            .map_or(full_summary.len(), |(idx, _)| idx);
        let merged_content_preview = full_summary[..preview_len].to_string();
        let mut merged_tags: Vec<String> = project_entries
            .iter()
            .flat_map(|entry| entry.tags.iter().cloned())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        merged_tags.sort();

        let group = MergeGroup {
            source_ids,
            merged_content_preview,
            full_summary,
            merged_tags,
        };

        plan.total_after_estimate = plan
            .total_after_estimate
            .saturating_sub(group.source_ids.len().saturating_sub(1));
        plan.groups_to_merge.push(group);
    }

    Ok(plan)
}

/// Execute consolidation (apply plan effects to store and index).
pub async fn execute_consolidation(
    store: &MemoryStore,
    index: Option<&MemoryIndex>,
    client: &dyn MemoryLlmClient,
) -> Result<ConsolidationPlan> {
    let plan = plan_consolidation(store, client).await?;
    if plan.groups_to_merge.is_empty() && plan.entries_to_expire.is_empty() {
        return Ok(plan);
    }

    let mut entries = store.load_all()?;
    let now = Utc::now();

    for entry_id in &plan.entries_to_expire {
        if let Some(entry) = entries
            .iter_mut()
            .find(|item| item.id.to_string() == *entry_id)
        {
            entry.valid_until = Some(now);
        }
    }

    for group in &plan.groups_to_merge {
        let project = entries.iter().find_map(|entry| {
            let id = entry.id.to_string();
            group
                .source_ids
                .iter()
                .any(|source_id| source_id == &id)
                .then(|| entry.project.clone())
                .flatten()
        });

        for source_id in &group.source_ids {
            if let Some(entry) = entries
                .iter_mut()
                .find(|item| item.id.to_string() == *source_id)
            {
                entry.valid_until = Some(now);
            }
        }

        let consolidated = MemoryEntry {
            id: Ulid::new(),
            timestamp: now,
            project,
            tool: None,
            session_id: None,
            tags: group.merged_tags.clone(),
            content: group.full_summary.clone(),
            facts: Vec::new(),
            source: MemorySource::Consolidated,
            valid_from: Some(now),
            valid_until: None,
        };
        entries.push(consolidated);
    }

    store.rewrite_all(&entries)?;

    if let Some(idx) = index {
        let active: Vec<MemoryEntry> = entries
            .into_iter()
            .filter(|entry| entry.valid_until.is_none_or(|until| until > now))
            .collect();
        idx.rebuild(&active)?;
    }

    Ok(plan)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use anyhow::Result;
    use async_trait::async_trait;
    use chrono::Utc;
    use ulid::Ulid;

    use crate::{Fact, MemoryEntry, MemoryLlmClient, MemorySource, MemoryStore, NoopClient};

    use super::{execute_consolidation, plan_consolidation};

    fn make_test_store() -> MemoryStore {
        let dir =
            std::env::temp_dir().join(format!("csa-memory-consolidation-test-{}", Ulid::new()));
        MemoryStore::new(dir)
    }

    fn make_entry(content: String, project: &str) -> MemoryEntry {
        MemoryEntry {
            id: Ulid::new(),
            timestamp: Utc::now(),
            project: Some(project.to_string()),
            tool: Some("codex".to_string()),
            session_id: Some(format!("session-{}", Ulid::new())),
            tags: vec!["memory".to_string(), "test".to_string()],
            content,
            facts: Vec::new(),
            source: MemorySource::PostRun,
            valid_from: Some(Utc::now()),
            valid_until: None,
        }
    }

    #[tokio::test]
    async fn test_plan_consolidation_few_entries() -> Result<()> {
        let store = make_test_store();
        let client = NoopClient;

        for idx in 0..9 {
            store.append(&make_entry(format!("entry-{idx}"), "project-a"))?;
        }

        let plan = plan_consolidation(&store, &client).await?;
        assert_eq!(plan.total_before, 9);
        assert_eq!(plan.total_after_estimate, 9);
        assert!(plan.groups_to_merge.is_empty());
        assert!(plan.entries_to_expire.is_empty());

        fs::remove_dir_all(store.base_dir()).ok();
        Ok(())
    }

    #[tokio::test]
    async fn test_consolidation_marks_valid_until() -> Result<()> {
        let store = make_test_store();
        let client = NoopClient;

        for idx in 0..10 {
            store.append(&make_entry(format!("entry-{idx}"), "project-a"))?;
        }

        let plan = execute_consolidation(&store, None, &client).await?;
        assert_eq!(plan.groups_to_merge.len(), 1);

        let raw_file = store.base_dir().join("memories.jsonl");
        let raw = fs::read_to_string(&raw_file)?;
        let all_entries: Vec<MemoryEntry> = raw
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(serde_json::from_str)
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let expired_count = all_entries
            .iter()
            .filter(|entry| entry.valid_until.is_some())
            .count();
        assert_eq!(expired_count, 10);
        assert!(
            all_entries
                .iter()
                .any(|entry| matches!(entry.source, MemorySource::Consolidated)
                    && entry.valid_until.is_none())
        );
        assert_eq!(store.load_all()?.len(), 1);

        fs::remove_dir_all(store.base_dir()).ok();
        Ok(())
    }

    #[derive(Debug, Clone)]
    struct FixedSummaryClient {
        summary: String,
    }

    #[async_trait]
    impl MemoryLlmClient for FixedSummaryClient {
        async fn extract_facts(&self, _text: &str) -> Result<Vec<Fact>> {
            Ok(Vec::new())
        }

        async fn summarize(&self, _entries: &[MemoryEntry]) -> Result<String> {
            Ok(self.summary.clone())
        }
    }

    #[tokio::test]
    async fn test_execute_consolidation_persists_full_summary() -> Result<()> {
        let store = make_test_store();
        let full_summary = "a".repeat(260);
        let client = FixedSummaryClient {
            summary: full_summary.clone(),
        };

        for idx in 0..10 {
            store.append(&make_entry(format!("entry-{idx}"), "project-a"))?;
        }

        let plan = execute_consolidation(&store, None, &client).await?;
        assert_eq!(plan.groups_to_merge.len(), 1);
        assert_eq!(plan.groups_to_merge[0].merged_content_preview.len(), 200);
        assert_eq!(plan.groups_to_merge[0].full_summary, full_summary);

        let active_entries = store.load_all()?;
        assert_eq!(active_entries.len(), 1);
        let consolidated = active_entries
            .iter()
            .find(|entry| matches!(entry.source, MemorySource::Consolidated))
            .expect("consolidated entry should exist");
        assert_eq!(consolidated.content, full_summary);

        fs::remove_dir_all(store.base_dir()).ok();
        Ok(())
    }
}
