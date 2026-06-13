//! Hermes-backed xurl provider adapter.

#[path = "xurl_hermes/db.rs"]
mod db;
#[path = "xurl_hermes/render.rs"]
mod render;

#[cfg(test)]
#[path = "xurl_hermes/tests.rs"]
mod tests;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;

const PROVIDER: &str = "hermes";
const PREVIEW_WIDTH: usize = 80;
const SEARCH_CONTEXT_LINES: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CwdMatchKind {
    Exact,
    Ancestor,
    Descendant,
}

impl CwdMatchKind {
    fn rank(self) -> u8 {
        match self {
            Self::Exact => 0,
            Self::Ancestor | Self::Descendant => 1,
        }
    }
}

#[derive(Debug, Clone)]
struct HermesSession {
    id: String,
    title: Option<String>,
    cwd: Option<PathBuf>,
    source: String,
    started_at: f64,
    ended_at: Option<f64>,
    message_count: i64,
    match_kind: Option<CwdMatchKind>,
    preview: Option<String>,
}

impl HermesSession {
    fn updated_epoch(&self) -> f64 {
        self.ended_at.unwrap_or(self.started_at)
    }

    fn updated_at(&self) -> Option<String> {
        format_unix_seconds(self.ended_at.or(Some(self.started_at)))
    }
}

#[derive(Debug, Clone)]
struct HermesMessage {
    role: String,
    content: String,
}

#[derive(Debug, Clone)]
struct HermesPaths {
    state_db: PathBuf,
    cwd: PathBuf,
}

pub(crate) struct HermesThreadArgs {
    pub(crate) keyword: Option<String>,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) hermes_home: Option<PathBuf>,
    pub(crate) hermes_profile: Option<String>,
    pub(crate) limit: usize,
    pub(crate) json: bool,
}

pub(crate) struct HermesRecallArgs {
    pub(crate) keyword: Option<String>,
    pub(crate) session: Option<String>,
    pub(crate) page: Option<u32>,
    pub(crate) list: bool,
    pub(crate) all: bool,
    pub(crate) limit: usize,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) hermes_home: Option<PathBuf>,
    pub(crate) hermes_profile: Option<String>,
}

pub(crate) fn handle_threads(args: HermesThreadArgs) -> Result<()> {
    let HermesThreadArgs {
        keyword,
        cwd,
        hermes_home,
        hermes_profile,
        limit,
        json,
    } = args;

    if limit == 0 {
        anyhow::bail!("--limit must be greater than 0");
    }

    let paths = db::resolve_paths(
        cwd.as_deref(),
        hermes_home.as_deref(),
        hermes_profile.as_deref(),
    )?;
    let conn = db::open_state_db(&paths.state_db)?;
    let sessions = db::collect_sessions(
        &conn,
        &paths.cwd,
        false,
        keyword.as_deref(),
        limit,
        &paths.state_db,
    )?;

    if json {
        println!("{}", render::threads_json(&sessions, &paths.state_db)?);
    } else {
        render::print_thread_table(&sessions);
    }
    Ok(())
}

pub(crate) fn handle_recall(args: HermesRecallArgs) -> Result<()> {
    let HermesRecallArgs {
        keyword,
        session,
        page,
        list,
        all,
        limit,
        cwd,
        hermes_home,
        hermes_profile,
    } = args;

    if limit == 0 {
        anyhow::bail!("--limit must be greater than 0");
    }

    let paths = db::resolve_paths(
        cwd.as_deref(),
        hermes_home.as_deref(),
        hermes_profile.as_deref(),
    )?;
    let conn = db::open_state_db(&paths.state_db)?;

    if list {
        let sessions = db::collect_sessions(&conn, &paths.cwd, all, None, limit, &paths.state_db)?;
        render::print_recall_list(&sessions);
        return Ok(());
    }

    if let Some(keyword) = keyword {
        let terms = db::split_keyword_terms(&keyword)?;
        if let Some(selector) = session.as_deref() {
            let selected = db::select_session(&conn, selector, &paths.cwd, all, &paths.state_db)?;
            return render::print_in_session_matches(&conn, &selected, &terms, &paths.state_db);
        }

        let sessions = db::collect_sessions(
            &conn,
            &paths.cwd,
            all,
            Some(&terms.join(" ")),
            limit,
            &paths.state_db,
        )?;
        if sessions.is_empty() {
            let scope = if all {
                "all Hermes sessions".to_string()
            } else {
                format!("Hermes sessions matching cwd {}", paths.cwd.display())
            };
            println!("No matches for keyword '{}' in {scope}.", terms.join(" "));
        } else {
            render::print_keyword_hits(&sessions);
        }
        return Ok(());
    }

    let selector = session.as_deref().unwrap_or("latest");
    let selected = db::select_session(&conn, selector, &paths.cwd, all, &paths.state_db)?;
    let markdown = render::render_session_markdown(&conn, &selected, &paths.state_db)?;
    if let Some(page_n) = page {
        let pages = crate::recall_cmd::pages::split_markdown_pages(&markdown);
        let total = pages.len();
        let idx =
            crate::recall_cmd::pages::resolve_page_index(page_n, total).with_context(|| {
                format!(
                    "Page {page_n} is out of range: session has {total} page(s) \
                 (0 = current/newest, {} = oldest)",
                    total.saturating_sub(1)
                )
            })?;
        print!("{}", pages[idx]);
    } else if let Some(message) =
        full_session_output_guard_message(&selected.id, &markdown, std::io::stdout().is_terminal())
    {
        println!("{message}");
    } else {
        print!("{markdown}");
    }
    Ok(())
}

fn full_session_output_guard_message(
    session_id: &str,
    markdown: &str,
    stdout_is_terminal: bool,
) -> Option<String> {
    if !stdout_is_terminal {
        return None;
    }
    let command = format!("csa xurl recall --provider hermes --session {session_id}");
    crate::recall_cmd::output_guard_message_for_command(&command, markdown)
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

fn match_kind_display(kind: Option<CwdMatchKind>) -> &'static str {
    match kind {
        Some(CwdMatchKind::Exact) => "exact",
        Some(CwdMatchKind::Ancestor) => "ancestor",
        Some(CwdMatchKind::Descendant) => "descendant",
        None => "-",
    }
}

fn thread_source(db_path: &Path, session_id: &str) -> String {
    format!("{}#session={session_id}", db_path.display())
}

fn yaml_single_quoted(value: &str) -> String {
    value.replace('\'', "''")
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

fn format_unix_seconds(timestamp: Option<f64>) -> Option<String> {
    let value = timestamp?;
    if !value.is_finite() {
        return None;
    }
    let mut secs = value.trunc() as i64;
    let mut nanos = ((value.fract().abs()) * 1_000_000_000.0).round() as u32;
    if nanos >= 1_000_000_000 {
        secs = secs.saturating_add(1);
        nanos = 0;
    }
    let datetime: DateTime<Utc> = DateTime::from_timestamp(secs, nanos)?;
    Some(datetime.to_rfc3339())
}

fn normalize_path(path: &Path, base: &Path) -> PathBuf {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            std::path::Component::RootDir => normalized.push(component.as_os_str()),
            std::path::Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}
