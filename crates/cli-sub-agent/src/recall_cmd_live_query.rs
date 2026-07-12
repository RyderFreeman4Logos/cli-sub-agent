/// Live-query xurl for the current project's main-agent session.
///
/// This bypasses the history file entirely, probing each provider for
/// the most recent main thread that belongs to `project_root`. Returns
/// `None` when no provider has a matching session.
/// How many threads to fetch per provider when searching for a
/// project-matching session.  The most-recent thread may belong to a
/// different project, so we scan a small window.
const LIVE_QUERY_SCAN_LIMIT: usize = 20;

fn live_query_main_session(project_root: &Path) -> Option<SessionRef> {
    let roots = provider_roots().ok()?;
    for &provider in RECALL_PROVIDERS {
        if let Some(session_ref) =
            live_query_main_session_for_provider_with_roots(project_root, provider, &roots)
        {
            return Some(session_ref);
        }
    }
    None
}

fn live_query_main_session_for_provider(
    project_root: &Path,
    provider: xurl_core::ProviderKind,
) -> Option<SessionRef> {
    let roots = provider_roots().ok()?;
    live_query_main_session_for_provider_with_roots(project_root, provider, &roots)
}

fn live_query_main_session_for_provider_with_roots(
    project_root: &Path,
    provider: xurl_core::ProviderKind,
    roots: &xurl_core::ProviderRoots,
) -> Option<SessionRef> {
    let query = xurl_core::ThreadQuery {
        uri: format!("{}://", provider),
        provider,
        role: Some("main".to_string()),
        q: None,
        limit: LIVE_QUERY_SCAN_LIMIT,
        ignored_params: Vec::new(),
    };
    let Ok(result) = xurl_core::query_threads(&query, roots) else {
        return None;
    };
    for thread in &result.items {
        if thread_belongs_to_project(&thread.thread_source, project_root, provider) {
            return Some(SessionRef {
                sid: thread.thread_id.clone(),
                provider: provider.to_string(),
            });
        }
    }
    None
}

fn latest_history_entry<'a>(
    entries: &'a [&'a RecallHistoryEntry],
) -> Option<&'a RecallHistoryEntry> {
    entries.iter().next_back().copied()
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

pub(super) fn render_session_markdown(session_ref: &SessionRef) -> Result<String> {
    resolve_session_thread(session_ref).map(|(_, content)| content)
}

pub(super) fn provider_roots() -> Result<xurl_core::ProviderRoots> {
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
    output_guard_message_for_command(&format!("csa recall read {session_id}"), content)
}

pub(crate) fn output_guard_message_for_command(command: &str, content: &str) -> Option<String> {
    if content.len() < OUTPUT_GUARD_BYTES {
        return None;
    }

    let size_kb = content.len().div_ceil(1024);
    Some(format!(
        "OUTPUT_TOO_LARGE: {size_kb}KB. Use: {command} | tail -100"
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

pub(super) fn truncate_display(value: &str, width: usize) -> String {
    let mut chars = value.chars();
    let preview: String = chars.by_ref().take(width).collect();
    if chars.next().is_some() && width > 3 {
        format!("{}...", preview.chars().take(width - 3).collect::<String>())
    } else {
        preview
    }
}
