use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, params};

use super::{
    HermesMessage, HermesSession, PREVIEW_WIDTH, PROVIDER, SEARCH_CONTEXT_LINES, contains_ci,
    match_kind_display, thread_source, truncate_display, yaml_single_quoted,
};

pub(super) fn render_session_markdown(
    conn: &Connection,
    session: &HermesSession,
    db_path: &Path,
) -> Result<String> {
    let messages = load_render_messages(conn, &session.id)?;
    let uri = format!("hermes://{}", session.id);
    let source = thread_source(db_path, &session.id);

    let mut output = String::new();
    output.push_str("---\n");
    output.push_str(&format!("uri: '{}'\n", yaml_single_quoted(&uri)));
    output.push_str(&format!(
        "thread_source: '{}'\n",
        yaml_single_quoted(&source)
    ));
    if let Some(title) = session.title.as_deref() {
        output.push_str(&format!("title: '{}'\n", yaml_single_quoted(title)));
    }
    if let Some(cwd) = session.cwd.as_deref() {
        output.push_str(&format!(
            "cwd: '{}'\n",
            yaml_single_quoted(&cwd.display().to_string())
        ));
    }
    output.push_str("---\n\n# Thread\n\n## Timeline\n\n");

    if messages.is_empty() {
        output.push_str("_No user/assistant messages found._\n");
        return Ok(output);
    }

    for (index, message) in messages.iter().enumerate() {
        output.push_str(&format!(
            "## {}. {}\n\n{}\n\n",
            index + 1,
            heading_for_message(message),
            message.content.trim()
        ));
    }
    Ok(output)
}

fn load_render_messages(conn: &Connection, session_id: &str) -> Result<Vec<HermesMessage>> {
    let mut stmt = conn.prepare(
        "SELECT role, content \
         FROM messages \
         WHERE session_id = ?1 \
           AND COALESCE(active, 1) != 0 \
           AND content IS NOT NULL \
           AND TRIM(content) != '' \
           AND LOWER(role) IN ('user', 'assistant', 'system') \
         ORDER BY timestamp ASC, id ASC",
    )?;
    let rows = stmt.query_map(params![session_id], |row| {
        Ok(HermesMessage {
            role: row.get(0)?,
            content: row.get(1)?,
        })
    })?;

    let mut messages = Vec::new();
    for row in rows {
        messages.push(row?);
    }
    Ok(messages)
}

fn heading_for_message(message: &HermesMessage) -> &'static str {
    match message.role.to_ascii_lowercase().as_str() {
        "user" => "User",
        "assistant" => "Assistant",
        "system" if message.content.to_ascii_lowercase().contains("compact") => "Context Compacted",
        "system" => "System",
        _ => "Message",
    }
}

pub(super) fn print_in_session_matches(
    conn: &Connection,
    session: &HermesSession,
    terms: &[String],
    db_path: &Path,
) -> Result<()> {
    let content = render_session_markdown(conn, session, db_path)?;
    let lines: Vec<&str> = content.lines().collect();
    let ranges = matching_ranges_any(&lines, terms, SEARCH_CONTEXT_LINES);
    if ranges.is_empty() {
        println!(
            "No matches for '{}' in session {}.",
            terms.join(" "),
            session.id
        );
        return Ok(());
    }

    println!("Matches in session {} ({PROVIDER})", session.id);
    for (start, end) in ranges {
        println!("\n-- lines {}-{} --", start + 1, end + 1);
        for (idx, line) in lines.iter().enumerate().take(end + 1).skip(start) {
            let hit = terms.iter().any(|term| contains_ci(line, term));
            let marker = if hit { ">" } else { " " };
            println!("{marker} {:>5} {line}", idx + 1);
        }
    }
    Ok(())
}

fn matching_ranges_any(lines: &[&str], terms: &[String], context: usize) -> Vec<(usize, usize)> {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if !terms.iter().any(|term| contains_ci(line, term)) {
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

pub(super) fn print_thread_table(sessions: &[HermesSession]) {
    if sessions.is_empty() {
        println!("No Hermes threads found.");
        return;
    }
    println!(
        "{:<12} {:<36} {:<20} {:<12} PREVIEW",
        "PROVIDER", "THREAD_ID", "UPDATED", "CWD_MATCH"
    );
    println!("{}", "-".repeat(120));
    for session in sessions {
        let updated = session.updated_at().unwrap_or_else(|| "-".to_string());
        let preview = session
            .preview
            .clone()
            .or_else(|| session.title.clone())
            .unwrap_or_default();
        println!(
            "{:<12} {:<36} {:<20} {:<12} {}",
            PROVIDER,
            truncate_display(&session.id, 36),
            truncate_display(&updated, 20),
            match_kind_display(session.match_kind),
            truncate_display(&preview, 60),
        );
    }
    println!("\nTotal: {} thread(s)", sessions.len());
}

pub(super) fn print_recall_list(sessions: &[HermesSession]) {
    if sessions.is_empty() {
        println!("No Hermes recall sessions found.");
        return;
    }
    println!(
        "{:<6} {:<20} {:<36} {:<12} TITLE",
        "INDEX", "UPDATED", "SESSION", "CWD_MATCH"
    );
    println!("{}", "-".repeat(120));
    for (index, session) in sessions.iter().enumerate() {
        let updated = session.updated_at().unwrap_or_else(|| "-".to_string());
        println!(
            "{:<6} {:<20} {:<36} {:<12} {}",
            index + 1,
            truncate_display(&updated, 20),
            truncate_display(&session.id, 36),
            match_kind_display(session.match_kind),
            truncate_display(session.title.as_deref().unwrap_or(""), 40),
        );
    }
    println!("\nTotal history entries: {}", sessions.len());
}

pub(super) fn print_keyword_hits(sessions: &[HermesSession]) {
    println!(
        "{:<10} {:<36} {:<20} PREVIEW",
        "PROVIDER", "SESSION", "UPDATED"
    );
    println!("{}", "-".repeat(160));
    for session in sessions {
        let updated = session.updated_at().unwrap_or_else(|| "-".to_string());
        let preview = session.preview.as_deref().unwrap_or("");
        println!(
            "{:<10} {:<36} {:<20} {}",
            PROVIDER,
            truncate_display(&session.id, 36),
            truncate_display(&updated, 20),
            truncate_display(preview, PREVIEW_WIDTH),
        );
    }
    println!("\nTotal matches: {}", sessions.len());
}

pub(super) fn threads_json(sessions: &[HermesSession], db_path: &Path) -> Result<String> {
    let items = sessions
        .iter()
        .map(|session| {
            serde_json::json!({
                "thread_id": session.id,
                "provider": PROVIDER,
                "uri": format!("hermes://{}", session.id),
                "source": thread_source(db_path, &session.id),
                "updated_at": session.updated_at(),
                "preview": session.preview.as_deref().or(session.title.as_deref()),
                "title": session.title.as_deref(),
                "cwd": session.cwd.as_ref().map(|path| path.display().to_string()),
                "cwd_match": session.match_kind,
                "message_count": session.message_count,
                "session_source": session.source.as_str(),
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string_pretty(&items).context("Failed to serialize Hermes thread JSON")
}
