use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use csa_config::memory::MemoryConfig;
use csa_memory::{
    ApiClient, MemoryEntry, MemoryIndex, MemoryLlmClient, MemorySource, MemoryStore, NoopClient,
    SearchResult,
};
use ulid::Ulid;

const APP_NAME: &str = "cli-sub-agent";
const OUTPUT_TRUNCATE_CHARS: usize = 500;
const INJECT_MAX_RESULTS: usize = 5;
const INJECT_QUERY_MAX_CHARS: usize = 200;
const INJECT_FALLBACK_TERMS: usize = 10;
const INJECT_SNIPPET_MAX_CHARS: usize = 200;

/// Capture memory from a completed session.
pub async fn capture_session_memory(
    config: &MemoryConfig,
    session_dir: &Path,
    project_key: Option<&str>,
    tool: Option<&str>,
    session_id: Option<&str>,
) -> Result<()> {
    let memory_dir = resolve_memory_base_dir();
    let store = MemoryStore::new(memory_dir.clone());
    let index_dir = memory_dir.join("index");

    capture_session_memory_to_store(
        config,
        session_dir,
        project_key,
        tool,
        session_id,
        &store,
        &index_dir,
    )
    .await
}

async fn capture_session_memory_to_store(
    config: &MemoryConfig,
    session_dir: &Path,
    project_key: Option<&str>,
    tool: Option<&str>,
    session_id: Option<&str>,
    store: &MemoryStore,
    index_dir: &Path,
) -> Result<()> {
    if !config.auto_capture {
        return Ok(());
    }

    let summary = read_session_summary(session_dir)?;
    if summary.trim().is_empty() {
        return Ok(());
    }

    let client = create_llm_client(config);
    let facts = client.extract_facts(&summary).await?;
    let entry_id = Ulid::new();
    let now = chrono::Utc::now();
    let entry = MemoryEntry {
        id: entry_id,
        timestamp: now,
        project: project_key.map(str::to_string),
        tool: tool.map(str::to_string),
        session_id: session_id.map(str::to_string),
        tags: facts.iter().flat_map(|fact| fact.tags.clone()).collect(),
        content: summary,
        facts: facts.into_iter().map(|fact| fact.content).collect(),
        source: MemorySource::PostRun,
        valid_from: Some(now),
        valid_until: None,
    };

    store.append(&entry)?;

    match MemoryIndex::open(index_dir) {
        Ok(index) => {
            if let Err(err) = index.index_entry(&entry) {
                tracing::warn!(error = %err, "Failed to index memory entry");
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, "Failed to open memory index");
        }
    }

    tracing::info!(
        entry_id = %entry.id,
        project = ?entry.project,
        tool = ?entry.tool,
        facts_count = entry.facts.len(),
        "Memory captured from session"
    );

    Ok(())
}

pub(crate) fn build_memory_section(
    config: &MemoryConfig,
    prompt: &str,
    project_key: Option<&str>,
) -> Option<String> {
    let memory_dir = resolve_memory_base_dir();
    let store = MemoryStore::new(memory_dir.clone());
    let index_dir = memory_dir.join("index");
    build_memory_section_from_store(config, prompt, project_key, &store, &index_dir)
}

fn build_memory_section_from_store(
    config: &MemoryConfig,
    prompt: &str,
    project_key: Option<&str>,
    store: &MemoryStore,
    index_dir: &Path,
) -> Option<String> {
    let query: String = prompt.chars().take(INJECT_QUERY_MAX_CHARS).collect();
    if query.trim().is_empty() {
        return None;
    }

    let mut results = match MemoryIndex::open(index_dir) {
        Ok(index) => match index.search(&query, INJECT_MAX_RESULTS) {
            Ok(search_results) => search_results,
            Err(err) => {
                tracing::warn!(error = %err, "Memory BM25 search failed; falling back to quick_search");
                Vec::new()
            }
        },
        Err(err) => {
            tracing::debug!(error = %err, "Memory index unavailable; falling back to quick_search");
            Vec::new()
        }
    };

    if let Some(project_name) = project_key
        && let Ok(entries) = store.load_all()
    {
        let allowed_ids: std::collections::HashSet<String> = entries
            .into_iter()
            .filter(|entry| entry.project.as_deref() == Some(project_name))
            .map(|entry| entry.id.to_string())
            .collect();
        results.retain(|result| allowed_ids.contains(&result.entry_id));
    }

    if results.is_empty() {
        let fallback_query = prompt
            .split_whitespace()
            .take(INJECT_FALLBACK_TERMS)
            .collect::<Vec<_>>()
            .join(" ");
        if fallback_query.trim().is_empty() {
            return None;
        }

        let escaped = regex::escape(&fallback_query);
        results = store
            .quick_search(&escaped)
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| project_key.is_none_or(|name| entry.project.as_deref() == Some(name)))
            .take(INJECT_MAX_RESULTS)
            .map(|entry| SearchResult {
                entry_id: entry.id.to_string(),
                score: 1.0,
                snippet: entry
                    .content
                    .chars()
                    .take(INJECT_SNIPPET_MAX_CHARS)
                    .collect(),
            })
            .collect();
    }

    if results.is_empty() {
        return None;
    }

    let mut section = String::from("\n<!-- CSA:MEMORY -->\n");
    section.push_str("The following are relevant memories from previous sessions:\n\n");

    let mut token_estimate = 0u32;
    let mut appended = 0usize;
    for result in &results {
        let snippet = result.snippet.replace('\n', " ");
        if snippet.trim().is_empty() {
            continue;
        }

        let mut snippet_for_output = snippet;
        let mut entry_tokens = ((snippet_for_output.chars().count() as u32) / 4).max(1);
        if token_estimate.saturating_add(entry_tokens) > config.inject_token_budget {
            if appended > 0 {
                break;
            }

            let max_chars = (config.inject_token_budget.saturating_mul(4)) as usize;
            if max_chars == 0 {
                break;
            }

            snippet_for_output = snippet_for_output.chars().take(max_chars).collect();
            if snippet_for_output.trim().is_empty() {
                break;
            }
            entry_tokens = ((snippet_for_output.chars().count() as u32) / 4).max(1);
            if token_estimate.saturating_add(entry_tokens) > config.inject_token_budget {
                break;
            }
        }

        let short_id: String = result.entry_id.chars().take(8).collect();
        let display_id = if short_id.is_empty() {
            "unknown"
        } else {
            &short_id
        };
        section.push_str(&format!("- [{display_id}] {}\n", snippet_for_output.trim()));
        token_estimate = token_estimate.saturating_add(entry_tokens);
        appended += 1;
    }

    if appended == 0 {
        return None;
    }

    section.push_str("<!-- CSA:MEMORY:END -->\n");
    Some(section)
}

fn read_session_summary(session_dir: &Path) -> Result<String> {
    let summary_path = session_dir.join("output").join("summary.txt");
    if summary_path.is_file() {
        return fs::read_to_string(&summary_path)
            .with_context(|| format!("failed to read summary: {}", summary_path.display()));
    }

    let result_path = session_dir.join("result.toml");
    if result_path.is_file() {
        let content = fs::read_to_string(&result_path)
            .with_context(|| format!("failed to read result file: {}", result_path.display()))?;
        if let Ok(result) = toml::from_str::<toml::Value>(&content)
            && let Some(summary) = result.get("summary").and_then(toml::Value::as_str)
        {
            return Ok(summary.to_string());
        }
    }

    let output_path = session_dir.join("output.log");
    if output_path.is_file() {
        let content = fs::read_to_string(&output_path)
            .with_context(|| format!("failed to read output log: {}", output_path.display()))?;
        let truncated: String = content.chars().take(OUTPUT_TRUNCATE_CHARS).collect();
        return Ok(truncated);
    }

    Ok(String::new())
}

fn create_llm_client(config: &MemoryConfig) -> Box<dyn MemoryLlmClient> {
    if config.llm.enabled && !config.llm.base_url.is_empty() && !config.llm.models.is_empty() {
        match ApiClient::new(
            &config.llm.base_url,
            &config.llm.api_key,
            &config.llm.models,
        ) {
            Ok(client) => return Box::new(client),
            Err(err) => {
                tracing::warn!(error = %err, "Failed to create API client, falling back to noop");
            }
        }
    }

    Box::new(NoopClient)
}

fn resolve_memory_base_dir() -> PathBuf {
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

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::Utc;
    use tempfile::tempdir;
    use ulid::Ulid;

    fn test_memory_config(auto_capture: bool) -> MemoryConfig {
        MemoryConfig {
            auto_capture,
            ..MemoryConfig::default()
        }
    }

    fn make_entry(id: &str, project: Option<&str>, content: &str) -> MemoryEntry {
        MemoryEntry {
            id: id.parse::<Ulid>().expect("valid ULID"),
            timestamp: Utc::now(),
            project: project.map(str::to_string),
            tool: Some("codex".to_string()),
            session_id: Some(format!("session-{id}")),
            tags: vec!["test".to_string()],
            content: content.to_string(),
            facts: vec!["fact".to_string()],
            source: MemorySource::Manual,
            valid_from: None,
            valid_until: None,
        }
    }

    #[tokio::test]
    async fn test_capture_with_noop_client() {
        let session_dir = tempdir().expect("create temp session dir");
        let memory_dir = tempdir().expect("create temp memory dir");
        let output_path = session_dir.path().join("output.log");
        fs::write(&output_path, "Session completed with actionable output.")
            .expect("write output.log");

        let store = MemoryStore::new(memory_dir.path().to_path_buf());
        let index_dir = memory_dir.path().join("index");
        capture_session_memory_to_store(
            &test_memory_config(true),
            session_dir.path(),
            Some("test-project"),
            Some("codex"),
            Some("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
            &store,
            &index_dir,
        )
        .await
        .expect("capture should succeed");

        let memories_path = memory_dir.path().join("memories.jsonl");
        assert!(memories_path.is_file());

        let entries = store.load_all().expect("load entries");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].project.as_deref(), Some("test-project"));
        assert_eq!(entries[0].tool.as_deref(), Some("codex"));
        assert!(!entries[0].facts.is_empty());
    }

    #[tokio::test]
    async fn test_capture_disabled() {
        let session_dir = tempdir().expect("create temp session dir");
        let memory_dir = tempdir().expect("create temp memory dir");
        let output_path = session_dir.path().join("output.log");
        fs::write(&output_path, "This output should not be persisted.").expect("write output.log");

        let store = MemoryStore::new(memory_dir.path().to_path_buf());
        let index_dir = memory_dir.path().join("index");
        capture_session_memory_to_store(
            &test_memory_config(false),
            session_dir.path(),
            Some("test-project"),
            Some("codex"),
            Some("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
            &store,
            &index_dir,
        )
        .await
        .expect("capture should return ok when disabled");

        assert!(!memory_dir.path().join("memories.jsonl").exists());
    }

    #[tokio::test]
    async fn test_capture_generates_unique_entry_ids_for_same_session() {
        let session_dir = tempdir().expect("create temp session dir");
        let memory_dir = tempdir().expect("create temp memory dir");
        let output_path = session_dir.path().join("output.log");
        fs::write(&output_path, "Session output for duplicate-id regression test.")
            .expect("write output.log");

        let store = MemoryStore::new(memory_dir.path().to_path_buf());
        let index_dir = memory_dir.path().join("index");
        let session_id = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

        capture_session_memory_to_store(
            &test_memory_config(true),
            session_dir.path(),
            Some("test-project"),
            Some("codex"),
            Some(session_id),
            &store,
            &index_dir,
        )
        .await
        .expect("first capture should succeed");

        capture_session_memory_to_store(
            &test_memory_config(true),
            session_dir.path(),
            Some("test-project"),
            Some("codex"),
            Some(session_id),
            &store,
            &index_dir,
        )
        .await
        .expect("second capture should succeed");

        let entries = store.load_all().expect("load entries");
        assert_eq!(entries.len(), 2);
        assert_ne!(
            entries[0].id, entries[1].id,
            "entry id must be unique even when session_id repeats"
        );
        assert!(entries.iter().all(|entry| entry.session_id.as_deref() == Some(session_id)));
    }

    #[test]
    fn test_read_session_summary_from_output() {
        let session_dir = tempdir().expect("create temp session dir");
        let output_dir = session_dir.path().join("output");
        fs::create_dir_all(&output_dir).expect("create output dir");

        let summary_path = output_dir.join("summary.txt");
        fs::write(&summary_path, "preferred summary").expect("write summary");
        fs::write(
            session_dir.path().join("result.toml"),
            "summary = \"fallback summary\"",
        )
        .expect("write result.toml");
        fs::write(session_dir.path().join("output.log"), "fallback output")
            .expect("write output.log");

        let summary = read_session_summary(session_dir.path()).expect("read session summary");
        assert_eq!(summary, "preferred summary");
    }

    #[test]
    fn test_build_memory_section_empty() {
        let memory_dir = tempdir().expect("create temp memory dir");
        let store = MemoryStore::new(memory_dir.path().to_path_buf());
        let index_dir = memory_dir.path().join("index");
        let config = MemoryConfig {
            inject: true,
            ..MemoryConfig::default()
        };

        let section = build_memory_section_from_store(
            &config,
            "search for memory entries",
            Some("test-project"),
            &store,
            &index_dir,
        );
        assert!(section.is_none());
    }

    #[test]
    fn test_build_memory_section_with_entries() {
        let memory_dir = tempdir().expect("create temp memory dir");
        let store = MemoryStore::new(memory_dir.path().to_path_buf());
        let index_dir = memory_dir.path().join("index");
        let config = MemoryConfig {
            inject: true,
            inject_token_budget: 2000,
            ..MemoryConfig::default()
        };

        let entry = make_entry(
            "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            Some("test-project"),
            "session fixed token budget handling for memory injection",
        );
        store.append(&entry).expect("append memory entry");
        let index = MemoryIndex::open(&index_dir).expect("open memory index");
        index.index_entry(&entry).expect("index memory entry");

        let section = build_memory_section_from_store(
            &config,
            "token budget injection",
            Some("test-project"),
            &store,
            &index_dir,
        )
        .expect("memory section should be generated");

        assert!(section.contains("<!-- CSA:MEMORY -->"));
        assert!(section.contains("<!-- CSA:MEMORY:END -->"));
        assert!(section.contains("- [01ARZ3ND]"));
        assert!(section.contains("token budget handling"));
    }

    #[test]
    fn test_build_memory_section_token_budget() {
        let memory_dir = tempdir().expect("create temp memory dir");
        let store = MemoryStore::new(memory_dir.path().to_path_buf());
        let index_dir = memory_dir.path().join("index");
        let config = MemoryConfig {
            inject: true,
            inject_token_budget: 6,
            ..MemoryConfig::default()
        };

        let entry_a = make_entry(
            "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            Some("test-project"),
            "alpha memory one short",
        );
        let entry_b = make_entry(
            "01ARZ3NDEKTSV4RRFFQ69G5FAW",
            Some("test-project"),
            "alpha memory two short",
        );
        store.append(&entry_a).expect("append entry A");
        store.append(&entry_b).expect("append entry B");
        let index = MemoryIndex::open(&index_dir).expect("open memory index");
        index.index_entry(&entry_a).expect("index entry A");
        index.index_entry(&entry_b).expect("index entry B");

        let section = build_memory_section_from_store(
            &config,
            "alpha memory",
            Some("test-project"),
            &store,
            &index_dir,
        )
        .expect("memory section should be generated");

        let bullet_count = section
            .lines()
            .filter(|line| line.starts_with("- ["))
            .count();
        assert_eq!(bullet_count, 1, "token budget should limit to one memory");
    }
}
