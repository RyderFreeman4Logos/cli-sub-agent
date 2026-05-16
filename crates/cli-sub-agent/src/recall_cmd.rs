//! recall_cmd — main-agent session history tracking and xurl-based recovery.

#[path = "recall_cmd_pages.rs"]
mod pages;

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::cli::RecallCommands;

const HISTORY_FILE_NAME: &str = "main-agent-history.jsonl";
const OUTPUT_GUARD_BYTES: usize = 50 * 1024;
const RECENT_DEDUP_WINDOW: usize = 10;
const SEARCH_CONTEXT_LINES: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RecallHistoryEntry {
    ts: String,
    sid: String,
    project: String,
    provider: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionRef {
    sid: String,
    provider: String,
}

pub(crate) fn handle_recall(cmd: RecallCommands) -> Result<()> {
    match cmd {
        RecallCommands::List { limit } => handle_recall_list(limit),
        RecallCommands::Read { session, page } => handle_recall_read(&session, page),
        RecallCommands::Search { query } => handle_recall_search(&query),
        RecallCommands::Pages { session } => handle_recall_pages(&session),
    }
}

fn recall_allowed_at_depth(depth: u32) -> bool {
    depth == 0
}

fn thread_belongs_to_project(thread_source: &str, project_root: &Path) -> bool {
    let encoded = project_root.display().to_string().replace('/', "-");
    let Some(parent) = std::path::Path::new(thread_source).parent() else {
        return false;
    };
    let dir_name = parent
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    dir_name.as_ref() == encoded
}

pub(crate) fn spawn_recall_record_if_needed(project_root: &Path) {
    if !recall_allowed_at_depth(crate::pipeline_env::current_csa_depth()) {
        return;
    }

    let project_root = project_root.to_path_buf();
    tokio::spawn(async move {
        if let Err(err) = record_main_agent_session(&project_root).await {
            debug!("recall: background task: {err:#}");
        }
    });
}

pub(crate) async fn record_main_agent_session(project_root: &Path) -> Result<()> {
    let roots = provider_roots()?;
    let query = xurl_core::ThreadQuery {
        uri: format!("{}://", xurl_core::ProviderKind::Claude),
        provider: xurl_core::ProviderKind::Claude,
        role: Some("main".to_string()),
        q: None,
        limit: 1,
        ignored_params: Vec::new(),
    };

    let result =
        xurl_core::query_threads(&query, &roots).context("Failed to query Claude main threads")?;
    let Some(thread) = result.items.first() else {
        debug!("recall: no Claude main thread available");
        return Ok(());
    };

    if !thread_belongs_to_project(&thread.thread_source, project_root) {
        debug!(
            thread_source = %thread.thread_source,
            project = %project_root.display(),
            "recall: skipping — session belongs to a different project"
        );
        return Ok(());
    }

    let entry = RecallHistoryEntry {
        ts: Utc::now().to_rfc3339(),
        sid: thread.thread_id.clone(),
        project: project_root.display().to_string(),
        provider: xurl_core::ProviderKind::Claude.to_string(),
    };

    let history_path = history_path()?;
    let appended = append_history_entry(&history_path, &entry)?;
    debug!(
        sid = %entry.sid,
        provider = %entry.provider,
        appended,
        "recall: main-agent session tracked"
    );
    Ok(())
}

fn handle_recall_list(limit: usize) -> Result<()> {
    if limit == 0 {
        anyhow::bail!("--limit must be greater than 0");
    }

    let entries = load_history_entries(&history_path()?)?;
    if entries.is_empty() {
        println!("No recall history found.");
        return Ok(());
    }

    let recent: Vec<_> = entries.into_iter().rev().take(limit).collect();

    println!(
        "{:<6} {:<25} {:<12} {:<36} PROJECT",
        "INDEX", "TIMESTAMP", "PROVIDER", "SESSION"
    );
    println!("{}", "-".repeat(120));
    for (offset, entry) in recent.iter().enumerate() {
        println!(
            "{:<6} {:<25} {:<12} {:<36} {}",
            offset + 1,
            truncate_display(&entry.ts, 25),
            truncate_display(&entry.provider, 12),
            truncate_display(&entry.sid, 36),
            truncate_display(&entry.project, 40),
        );
    }
    println!("\nTotal history entries: {}", recent.len());
    Ok(())
}

fn handle_recall_read(session: &str, page: Option<i32>) -> Result<()> {
    let session_ref = resolve_session_ref(session)?;
    let content = render_session_markdown(&session_ref)?;

    let Some(page_n) = page else {
        if std::io::stdout().is_terminal()
            && let Some(message) = output_guard_message(&session_ref.sid, &content)
        {
            println!("{message}");
            return Ok(());
        }
        print!("{content}");
        return Ok(());
    };

    let page_list = pages::split_markdown_pages(&content);
    let total = page_list.len();
    let idx = pages::resolve_page_index(page_n, total).with_context(|| {
        format!(
            "Page {page_n} is out of range: session has {total} page(s) \
             (use negative values to count from the end)"
        )
    })?;
    print!("{}", page_list[idx]);
    Ok(())
}

fn handle_recall_pages(session: &str) -> Result<()> {
    let session_ref = resolve_session_ref(session)?;
    let (resolved, content) = resolve_session_thread(&session_ref)?;

    let page_list = pages::split_markdown_pages(&content);
    if page_list.is_empty() {
        println!("No content found in session {}.", session_ref.sid);
        return Ok(());
    }

    let timestamps = pages::extract_jsonl_compact_timestamps(&resolved.path);

    let mut line_cursor = 1usize;
    println!(
        "{:<6} {:<15} {:<22} {:<8} Note",
        "Page", "Lines", "Timestamp", "Size"
    );
    println!("{}", "-".repeat(70));
    for (i, page_content) in page_list.iter().enumerate() {
        let page_num = i + 1;
        let page_line_count = page_content.lines().count().max(1);
        let line_start = line_cursor;
        let line_end = line_cursor + page_line_count - 1;
        line_cursor = line_end + 1;

        let ts = timestamps
            .get(i)
            .and_then(Option::as_ref)
            .map_or_else(|| String::from("-"), |ts| pages::format_timestamp_short(ts));
        let size_kb = page_content.len().div_ceil(1024);
        let note = if i == 0 { "" } else { "(compact)" };

        println!(
            "{:<6} {:<15} {:<22} {:<8} {}",
            page_num,
            format!("{line_start}-{line_end}"),
            ts,
            format!("{size_kb}KB"),
            note,
        );
    }
    println!("\nTotal pages: {}", page_list.len());
    Ok(())
}

fn handle_recall_search(query: &str) -> Result<()> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        anyhow::bail!("Search query must not be empty");
    }

    let session_ref = latest_session_ref()?;
    let content = render_session_markdown(&session_ref)?;
    let lines: Vec<&str> = content.lines().collect();
    let ranges = matching_ranges(&lines, trimmed, SEARCH_CONTEXT_LINES);

    if ranges.is_empty() {
        println!("No matches found in latest session {}.", session_ref.sid);
        return Ok(());
    }

    println!(
        "Matches in session {} ({})",
        session_ref.sid, session_ref.provider
    );
    for (start, end) in ranges {
        println!("\n-- lines {}-{} --", start + 1, end + 1);
        for (line_idx, line) in lines.iter().enumerate().take(end + 1).skip(start) {
            let marker = if line.contains(trimmed) { ">" } else { " " };
            println!("{marker} {:>5} {line}", line_idx + 1);
        }
    }

    Ok(())
}

fn latest_session_ref() -> Result<SessionRef> {
    resolve_session_ref("latest")
}

fn resolve_session_ref(selector: &str) -> Result<SessionRef> {
    let trimmed = selector.trim();
    if trimmed.is_empty() {
        anyhow::bail!("Session selector must not be empty");
    }

    let entries = load_history_entries(&history_path()?)?;
    if trimmed.eq_ignore_ascii_case("latest") {
        return latest_history_entry(&entries)
            .map(entry_to_session_ref)
            .context("No recall history found");
    }

    if let Ok(index) = trimmed.parse::<usize>() {
        if index == 0 {
            anyhow::bail!("History index is 1-based; use 1 for the most recent session");
        }
        let entry = entries
            .iter()
            .rev()
            .nth(index - 1)
            .with_context(|| format!("History index {index} is out of range"))?;
        return Ok(entry_to_session_ref(entry));
    }

    Ok(SessionRef {
        sid: trimmed.to_string(),
        provider: xurl_core::ProviderKind::Claude.to_string(),
    })
}

fn latest_history_entry(entries: &[RecallHistoryEntry]) -> Option<&RecallHistoryEntry> {
    entries.iter().next_back()
}

fn entry_to_session_ref(entry: &RecallHistoryEntry) -> SessionRef {
    SessionRef {
        sid: entry.sid.clone(),
        provider: entry.provider.clone(),
    }
}

fn resolve_session_thread(session_ref: &SessionRef) -> Result<(xurl_core::ResolvedThread, String)> {
    let roots = provider_roots()?;
    let uri_str = format!("agents://{}/{}", session_ref.provider, session_ref.sid);
    let uri: xurl_core::AgentsUri = uri_str
        .parse()
        .with_context(|| format!("Invalid agents URI: {uri_str}"))?;
    let resolved = xurl_core::resolve_thread(&uri, &roots)
        .with_context(|| format!("Failed to resolve thread {uri_str}"))?;
    let content = xurl_core::render_thread_markdown(&uri, &resolved)
        .with_context(|| format!("Failed to render thread {uri_str}"))?;
    Ok((resolved, content))
}

fn render_session_markdown(session_ref: &SessionRef) -> Result<String> {
    resolve_session_thread(session_ref).map(|(_, content)| content)
}

fn provider_roots() -> Result<xurl_core::ProviderRoots> {
    xurl_core::ProviderRoots::from_env_or_home().context("Failed to resolve provider roots")
}

fn history_path() -> Result<PathBuf> {
    let state_dir =
        csa_config::paths::state_dir_write().context("Failed to determine CSA state directory")?;
    Ok(state_dir.join(HISTORY_FILE_NAME))
}

fn load_history_entries(history_path: &Path) -> Result<Vec<RecallHistoryEntry>> {
    let file = match OpenOptions::new().read(true).open(history_path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(err).with_context(|| format!("Failed to read {}", history_path.display()));
        }
    };

    let reader = BufReader::new(file);
    let mut entries = Vec::new();
    for line in reader.lines() {
        let line =
            line.with_context(|| format!("Failed to read line from {}", history_path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<RecallHistoryEntry>(&line) {
            Ok(entry) => entries.push(entry),
            Err(err) => debug!(error = %err, line, "recall: skipping malformed history line"),
        }
    }
    Ok(entries)
}

fn append_history_entry(history_path: &Path, entry: &RecallHistoryEntry) -> Result<bool> {
    if let Some(parent) = history_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    if recent_duplicate_exists(history_path, &entry.sid)? {
        return Ok(false);
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(history_path)
        .with_context(|| format!("Failed to open {}", history_path.display()))?;

    let line = serde_json::to_string(entry).context("Failed to serialize recall history entry")?;
    file.write_all(line.as_bytes())
        .with_context(|| format!("Failed to append {}", history_path.display()))?;
    file.write_all(b"\n")
        .with_context(|| format!("Failed to finalize {}", history_path.display()))?;

    Ok(true)
}

fn recent_duplicate_exists(history_path: &Path, sid: &str) -> Result<bool> {
    let contents = match fs::read_to_string(history_path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => {
            return Err(err).with_context(|| format!("Failed to read {}", history_path.display()));
        }
    };

    Ok(contents
        .lines()
        .rev()
        .take(RECENT_DEDUP_WINDOW)
        .filter_map(|line| serde_json::from_str::<RecallHistoryEntry>(line).ok())
        .any(|entry| entry.sid == sid))
}

fn output_guard_message(session_id: &str, content: &str) -> Option<String> {
    if content.len() < OUTPUT_GUARD_BYTES {
        return None;
    }

    let size_kb = content.len().div_ceil(1024);
    Some(format!(
        "OUTPUT_TOO_LARGE: {size_kb}KB. Use: csa recall read {session_id} | tail -100"
    ))
}

fn matching_ranges(lines: &[&str], query: &str, context: usize) -> Vec<(usize, usize)> {
    let mut ranges: Vec<(usize, usize)> = Vec::new();

    for (idx, line) in lines.iter().enumerate() {
        if !line.contains(query) {
            continue;
        }

        let start = idx.saturating_sub(context);
        let end = (idx + context).min(lines.len().saturating_sub(1));
        if let Some((_, previous_end)) = ranges.last_mut()
            && start <= *previous_end + 1
        {
            *previous_end = (*previous_end).max(end);
            continue;
        }
        ranges.push((start, end));
    }

    ranges
}

fn truncate_display(value: &str, width: usize) -> String {
    let mut chars = value.chars();
    let preview: String = chars.by_ref().take(width).collect();
    if chars.next().is_some() && width > 3 {
        format!("{}...", preview.chars().take(width - 3).collect::<String>())
    } else {
        preview
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(sid: &str) -> RecallHistoryEntry {
        RecallHistoryEntry {
            ts: "2026-05-08T17:48:14Z".to_string(),
            sid: sid.to_string(),
            project: "/tmp/project".to_string(),
            provider: "claude".to_string(),
        }
    }

    #[test]
    fn append_history_entry_writes_jsonl_line() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let history_path = temp_dir.path().join(HISTORY_FILE_NAME);

        let appended = append_history_entry(&history_path, &make_entry("sid-1")).expect("append");
        assert!(appended, "first append must write a line");

        let entries = load_history_entries(&history_path).expect("load");
        assert_eq!(entries, vec![make_entry("sid-1")]);
    }

    #[test]
    fn append_history_entry_skips_recent_duplicate_sid() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let history_path = temp_dir.path().join(HISTORY_FILE_NAME);

        append_history_entry(&history_path, &make_entry("sid-1")).expect("first append");
        let appended =
            append_history_entry(&history_path, &make_entry("sid-1")).expect("second append");

        assert!(!appended, "duplicate sid in recent window must be skipped");
        let entries = load_history_entries(&history_path).expect("load");
        assert_eq!(
            entries.len(),
            1,
            "duplicate append must not add a second line"
        );
    }

    #[test]
    fn append_history_entry_allows_duplicate_outside_recent_window() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let history_path = temp_dir.path().join(HISTORY_FILE_NAME);

        append_history_entry(&history_path, &make_entry("sid-1")).expect("first append");
        for index in 0..RECENT_DEDUP_WINDOW {
            let sid = format!("sid-{index}-other");
            append_history_entry(&history_path, &make_entry(&sid)).expect("filler append");
        }

        let appended =
            append_history_entry(&history_path, &make_entry("sid-1")).expect("late append");
        assert!(
            appended,
            "sid older than the dedup window must be recorded again"
        );
    }

    #[test]
    fn output_guard_triggers_at_threshold() {
        let content = "x".repeat(OUTPUT_GUARD_BYTES);
        let message = output_guard_message("sid-1", &content).expect("guard");
        assert!(message.contains("OUTPUT_TOO_LARGE"));
        assert!(message.contains("csa recall read sid-1 | tail -100"));
    }

    #[test]
    fn output_guard_allows_small_output() {
        let content = "x".repeat(OUTPUT_GUARD_BYTES - 1);
        assert!(output_guard_message("sid-1", &content).is_none());
    }

    #[test]
    fn matching_ranges_merges_overlapping_context() {
        let lines = vec!["0", "match one", "2", "match two", "4"];
        let ranges = matching_ranges(&lines, "match", 1);
        assert_eq!(ranges, vec![(0, 4)]);
    }

    #[test]
    fn recall_allowed_only_at_main_agent_depth() {
        assert!(
            recall_allowed_at_depth(0),
            "main agent (depth=0) must be recorded"
        );
        assert!(
            !recall_allowed_at_depth(1),
            "depth=1 child session must not be recorded"
        );
        assert!(
            !recall_allowed_at_depth(5),
            "deeply nested child (depth=5) must not be recorded"
        );
    }

    #[test]
    fn thread_belongs_to_matching_project() {
        let source = "/home/obj/.claude/projects/-home-obj-project-github-user-repo/abc.jsonl";
        let root = Path::new("/home/obj/project/github/user/repo");
        assert!(thread_belongs_to_project(source, root));
    }

    #[test]
    fn thread_rejects_different_project() {
        let source = "/home/obj/.claude/projects/-home-obj-project-github-user-other/abc.jsonl";
        let root = Path::new("/home/obj/project/github/user/repo");
        assert!(!thread_belongs_to_project(source, root));
    }
}
