use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use csa_config::{GlobalConfig, MemoryConfig, ProjectConfig};
use csa_memory::{
    ApiClient, MemoryEntry, MemoryFilter, MemoryIndex, MemoryLlmClient, MemorySource, MemoryStore,
    NoopClient, execute_consolidation, plan_consolidation,
};
use ulid::Ulid;

use crate::cli::MemoryCommands;

const APP_NAME: &str = "cli-sub-agent";

pub fn handle_memory_command(command: MemoryCommands) -> Result<()> {
    match command {
        MemoryCommands::Search { query, limit, json } => handle_search(&query, limit, json),
        MemoryCommands::List {
            project,
            tool,
            tag,
            since,
            json,
        } => handle_list(project, tool, tag, since, json),
        MemoryCommands::Add { content, tags } => handle_add(content, tags),
        MemoryCommands::Show { id } => handle_show(&id),
        MemoryCommands::Gc { days, dry_run } => handle_gc(days, dry_run),
        MemoryCommands::Reindex => handle_reindex(),
        MemoryCommands::Consolidate { dry_run } => handle_consolidate(dry_run),
    }
}

fn handle_search(query: &str, limit: usize, json: bool) -> Result<()> {
    if limit == 0 {
        if json {
            println!("[]");
        } else {
            println!("Memory Search Results (0 matches):");
        }
        return Ok(());
    }

    let store = memory_store();
    let all_entries = store.load_all()?;
    let entry_map: HashMap<String, MemoryEntry> = all_entries
        .into_iter()
        .map(|entry| (entry.id.to_string(), entry))
        .collect();

    let mut used_fallback = false;
    let mut unresolved = 0usize;

    let ranked_entries = match open_memory_index().and_then(|index| index.search(query, limit)) {
        Ok(results) => {
            let mut matched = Vec::with_capacity(results.len());
            for result in results {
                if let Some(entry) = entry_map.get(&result.entry_id).cloned() {
                    matched.push((Some(result.score), entry));
                } else {
                    unresolved += 1;
                }
            }
            matched
        }
        Err(error) => {
            used_fallback = true;
            eprintln!("Warning: memory index unavailable ({error}); using quick-search fallback.");

            store
                .quick_search(&regex::escape(query))?
                .into_iter()
                .take(limit)
                .map(|entry| (None, entry))
                .collect()
        }
    };

    if json {
        let entries: Vec<MemoryEntry> =
            ranked_entries.into_iter().map(|(_, entry)| entry).collect();
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    println!("Memory Search Results ({} matches):", ranked_entries.len());
    if ranked_entries.is_empty() {
        return Ok(());
    }
    println!();

    for (idx, (score, entry)) in ranked_entries.iter().enumerate() {
        let score_str = score.map_or_else(|| "--".to_string(), |value| format!("{value:.2}"));
        println!(
            "#{} [{}] {}  {}  [{}] [{}]",
            idx + 1,
            score_str,
            short_id(&entry.id.to_string(), 8),
            format_timestamp(entry.timestamp),
            entry.tool.as_deref().unwrap_or("-"),
            entry.project.as_deref().unwrap_or("-")
        );
        println!("   {}", truncate_chars(&entry.content, 80));
        println!();
    }

    if unresolved > 0 {
        eprintln!(
            "Warning: skipped {unresolved} stale index hit(s) not found in memories.jsonl. Run `csa memory reindex`."
        );
    }
    if used_fallback {
        eprintln!("Hint: run `csa memory reindex` to restore BM25 ranking.");
    }

    Ok(())
}

fn handle_list(
    project: Option<String>,
    tool: Option<String>,
    tag: Option<String>,
    since: Option<String>,
    json: bool,
) -> Result<()> {
    let filter = MemoryFilter {
        project,
        tool,
        since: parse_since_date(since)?,
        tag,
    };

    let entries = memory_store().list(filter)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    if entries.is_empty() {
        println!("No memory entries found.");
        return Ok(());
    }

    println!(
        "{:<8}  {:<16}  {:<16}  {:<12}  {:<20}  CONTENT",
        "ID", "TIMESTAMP", "PROJECT", "TOOL", "TAGS"
    );
    for entry in entries {
        let tags = if entry.tags.is_empty() {
            "-".to_string()
        } else {
            truncate_chars(&entry.tags.join(","), 20)
        };
        println!(
            "{:<8}  {:<16}  {:<16}  {:<12}  {:<20}  {}",
            short_id(&entry.id.to_string(), 8),
            format_timestamp(entry.timestamp),
            truncate_chars(entry.project.as_deref().unwrap_or("-"), 16),
            truncate_chars(entry.tool.as_deref().unwrap_or("-"), 12),
            tags,
            truncate_chars(&entry.content, 60)
        );
    }

    Ok(())
}

fn handle_add(content: String, tags: Option<String>) -> Result<()> {
    let entry = MemoryEntry {
        id: Ulid::new(),
        timestamp: Utc::now(),
        project: detect_project_name(),
        tool: Some("manual".to_string()),
        session_id: None,
        tags: parse_tags(tags),
        content,
        facts: Vec::new(),
        source: MemorySource::Manual,
        valid_from: None,
        valid_until: None,
    };

    let store = memory_store();
    store.append(&entry)?;

    if let Err(error) = open_memory_index().and_then(|index| index.index_entry(&entry)) {
        bail!("memory entry saved but failed to update index: {error}. Run `csa memory reindex`.");
    }

    println!(
        "Added memory entry {} at {}.",
        short_id(&entry.id.to_string(), 8),
        entry.timestamp.to_rfc3339()
    );
    Ok(())
}

fn handle_show(id_prefix: &str) -> Result<()> {
    let entries = memory_store().load_all()?;
    let entry = resolve_by_prefix(&entries, id_prefix)?;

    println!("ID: {}", entry.id);
    println!("Timestamp: {}", entry.timestamp.to_rfc3339());
    println!("Project: {}", entry.project.as_deref().unwrap_or("-"));
    println!("Tool: {}", entry.tool.as_deref().unwrap_or("-"));
    println!("Session: {}", entry.session_id.as_deref().unwrap_or("-"));
    println!("Source: {:?}", entry.source);
    println!(
        "Valid From: {}",
        entry
            .valid_from
            .map(|value| value.to_rfc3339())
            .unwrap_or_else(|| "-".to_string())
    );
    println!(
        "Valid Until: {}",
        entry
            .valid_until
            .map(|value| value.to_rfc3339())
            .unwrap_or_else(|| "-".to_string())
    );

    if entry.tags.is_empty() {
        println!("Tags: -");
    } else {
        println!("Tags: {}", entry.tags.join(", "));
    }

    if entry.facts.is_empty() {
        println!("Facts: -");
    } else {
        println!("Facts:");
        for fact in &entry.facts {
            println!("  - {fact}");
        }
    }

    println!("Content:");
    println!("{}", entry.content);
    Ok(())
}

fn handle_gc(days: u32, dry_run: bool) -> Result<()> {
    let cutoff = Utc::now() - Duration::days(i64::from(days));
    let entries = memory_store().load_all()?;

    let mut keep = Vec::new();
    let mut remove = Vec::new();
    for entry in entries {
        if entry.timestamp < cutoff {
            remove.push(entry);
        } else {
            keep.push(entry);
        }
    }

    if dry_run {
        println!(
            "GC preview: {} entries would be removed, {} kept (older than {} days; cutoff {}).",
            remove.len(),
            keep.len(),
            days,
            cutoff.to_rfc3339()
        );
        return Ok(());
    }

    memory_store().rewrite_all(&keep)?;
    open_memory_index()?.rebuild(&keep)?;

    println!(
        "GC complete: removed {} entries, kept {} (cutoff {}).",
        remove.len(),
        keep.len(),
        cutoff.to_rfc3339()
    );
    Ok(())
}

fn handle_reindex() -> Result<()> {
    let entries = memory_store().load_all()?;
    open_memory_index()?.rebuild(&entries)?;
    println!("Rebuilt memory index from {} entries.", entries.len());
    Ok(())
}

fn handle_consolidate(dry_run: bool) -> Result<()> {
    let store = memory_store();
    let config = load_memory_config();
    let client = create_llm_client(&config);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to create tokio runtime for memory consolidation")?;

    if dry_run {
        let plan = runtime.block_on(plan_consolidation(&store, client.as_ref()))?;
        println!("Consolidation Plan:");
        println!("  Entries before: {}", plan.total_before);
        println!("  Entries after:  {}", plan.total_after_estimate);
        println!("  Groups to merge: {}", plan.groups_to_merge.len());
        for (idx, group) in plan.groups_to_merge.iter().enumerate() {
            let preview = truncate_chars(&group.merged_content_preview, 80);
            println!(
                "  Group {}: {} entries -> 1",
                idx + 1,
                group.source_ids.len()
            );
            println!("    Preview: {preview}");
        }
        return Ok(());
    }

    let index_dir = store.base_dir().join("index");
    let index = MemoryIndex::open(&index_dir).ok();
    let plan = runtime.block_on(execute_consolidation(
        &store,
        index.as_ref(),
        client.as_ref(),
    ))?;
    println!("Consolidation complete:");
    println!("  Entries before: {}", plan.total_before);
    println!("  Groups merged: {}", plan.groups_to_merge.len());
    println!("  Entries after (estimated): {}", plan.total_after_estimate);
    Ok(())
}

fn resolve_by_prefix<'a>(entries: &'a [MemoryEntry], prefix: &str) -> Result<&'a MemoryEntry> {
    let normalized = prefix.to_ascii_lowercase();
    let matches: Vec<&MemoryEntry> = entries
        .iter()
        .filter(|entry| {
            entry
                .id
                .to_string()
                .to_ascii_lowercase()
                .starts_with(&normalized)
        })
        .collect();

    match matches.as_slice() {
        [] => bail!("No memory entry matching prefix '{prefix}'."),
        [entry] => Ok(*entry),
        many => {
            let choices = many
                .iter()
                .map(|entry| short_id(&entry.id.to_string(), 10))
                .collect::<Vec<_>>()
                .join(", ");
            bail!("Ambiguous prefix '{prefix}'. Matches: {choices}");
        }
    }
}

fn parse_since_date(since: Option<String>) -> Result<Option<DateTime<Utc>>> {
    let Some(raw) = since else {
        return Ok(None);
    };

    let date = NaiveDate::parse_from_str(&raw, "%Y-%m-%d")
        .with_context(|| format!("invalid --since date '{raw}' (expected YYYY-MM-DD)"))?;
    let midnight = date
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| anyhow::anyhow!("failed to build midnight datetime for '{raw}'"))?;
    Ok(Some(DateTime::<Utc>::from_naive_utc_and_offset(
        midnight, Utc,
    )))
}

fn parse_tags(tags: Option<String>) -> Vec<String> {
    tags.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn detect_project_name() -> Option<String> {
    let project_root = crate::pipeline::determine_project_root(None).ok()?;
    let config = ProjectConfig::load(&project_root).ok()??;
    Some(config.project.name)
}

fn load_memory_config() -> MemoryConfig {
    let project_memory = crate::pipeline::determine_project_root(None)
        .ok()
        .and_then(|project_root| ProjectConfig::load(&project_root).ok().flatten())
        .map(|config| config.memory);
    if let Some(memory) = project_memory {
        return memory;
    }

    GlobalConfig::load()
        .map(|config| config.memory)
        .unwrap_or_default()
}

fn create_llm_client(config: &MemoryConfig) -> Box<dyn MemoryLlmClient> {
    if config.llm.enabled && !config.llm.base_url.is_empty() && !config.llm.models.is_empty() {
        match ApiClient::new(
            &config.llm.base_url,
            &config.llm.api_key,
            &config.llm.models,
        ) {
            Ok(client) => return Box::new(client),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "failed to initialize memory API client; falling back to noop"
                );
            }
        }
    }

    Box::new(NoopClient)
}

fn memory_store() -> MemoryStore {
    MemoryStore::new(resolve_memory_base_dir())
}

fn open_memory_index() -> Result<MemoryIndex> {
    MemoryIndex::open(&resolve_memory_base_dir().join("index"))
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

fn short_id(id: &str, len: usize) -> String {
    id.chars().take(len).collect()
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut truncated: String = value.chars().take(max_chars).collect();
    truncated.push_str("...");
    truncated
}

fn format_timestamp(ts: DateTime<Utc>) -> String {
    ts.format("%Y-%m-%d %H:%M").to_string()
}
