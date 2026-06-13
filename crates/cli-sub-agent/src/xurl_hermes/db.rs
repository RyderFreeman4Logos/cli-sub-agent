use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::BaseDirs;
use rusqlite::{Connection, OpenFlags, params};

use super::{
    CwdMatchKind, HermesPaths, HermesSession, PREVIEW_WIDTH, contains_ci, normalize_path,
    truncate_display,
};

pub(super) fn resolve_paths(
    cwd: Option<&Path>,
    hermes_home: Option<&Path>,
    hermes_profile: Option<&str>,
) -> Result<HermesPaths> {
    let current_dir = env::current_dir().context("Failed to determine current directory")?;
    let cwd = normalize_path(cwd.unwrap_or(&current_dir), &current_dir);
    let state_db = resolve_state_db_path(hermes_home, hermes_profile)?;
    Ok(HermesPaths { state_db, cwd })
}

fn resolve_state_db_path(
    hermes_home: Option<&Path>,
    hermes_profile: Option<&str>,
) -> Result<PathBuf> {
    let home = if let Some(home) = hermes_home {
        home.to_path_buf()
    } else if let Some(home) = env::var_os("HERMES_HOME").filter(|v| !v.is_empty()) {
        PathBuf::from(home)
    } else {
        let base_dirs = BaseDirs::new().context("Failed to determine home directory")?;
        base_dirs.home_dir().join(".hermes")
    };

    if home.is_file() {
        return Ok(home);
    }

    let Some(profile) = hermes_profile.filter(|value| !value.trim().is_empty()) else {
        return Ok(home.join("state.db"));
    };

    let profile = profile.trim();
    let candidates = [
        home.join(profile).join("state.db"),
        home.join("profiles").join(profile).join("state.db"),
        home.join(format!("state.{profile}.db")),
    ];
    Ok(candidates
        .iter()
        .find(|path| path.exists())
        .cloned()
        .unwrap_or_else(|| candidates[0].clone()))
}

pub(super) fn open_state_db(path: &Path) -> Result<Connection> {
    if !path.exists() {
        anyhow::bail!(
            "Hermes state DB not found at {}. Set HERMES_HOME or pass --hermes-home.",
            path.display()
        );
    }

    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| {
        format!(
            "Failed to open Hermes state DB read-only at {}",
            path.display()
        )
    })?;
    conn.pragma_update(None, "query_only", "ON")
        .with_context(|| {
            format!(
                "Failed to mark Hermes state DB connection query-only at {}",
                path.display()
            )
        })?;
    ensure_schema(&conn, path)?;
    Ok(conn)
}

fn ensure_schema(conn: &Connection, path: &Path) -> Result<()> {
    for table in ["sessions", "messages"] {
        if !sqlite_object_exists(conn, table)? {
            anyhow::bail!(
                "Hermes state DB {} is missing required table '{table}'",
                path.display()
            );
        }
    }
    Ok(())
}

pub(super) fn collect_sessions(
    conn: &Connection,
    cwd: &Path,
    all: bool,
    keyword: Option<&str>,
    limit: usize,
    db_path: &Path,
) -> Result<Vec<HermesSession>> {
    let mut sessions = load_sessions(conn)?;
    if !all {
        attach_cwd_matches(&mut sessions, cwd);
        sessions.retain(|session| session.match_kind.is_some());
    }

    if let Some(keyword) = keyword {
        let terms = split_keyword_terms(keyword)?;
        let fts_available = sqlite_object_exists(conn, "messages_fts")?;
        let mut filtered = Vec::new();
        for mut session in sessions {
            if let Some(preview) = search_session_preview(conn, &session.id, &terms, fts_available)?
            {
                session.preview = Some(preview);
                filtered.push(session);
            }
        }
        sessions = filtered;
    }

    sort_sessions(&mut sessions);
    sessions.truncate(limit);

    if sessions.is_empty() && !all {
        tracing::debug!(cwd = %cwd.display(), db = %db_path.display(), "no Hermes sessions matched cwd");
    }
    Ok(sessions)
}

fn load_sessions(conn: &Connection) -> Result<Vec<HermesSession>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, cwd, source, started_at, ended_at, COALESCE(message_count, 0) \
         FROM sessions \
         WHERE COALESCE(archived, 0) = 0",
    )?;
    let rows = stmt.query_map([], |row| {
        let cwd_raw: Option<String> = row.get(2)?;
        Ok(HermesSession {
            id: row.get(0)?,
            title: row.get(1)?,
            cwd: cwd_raw
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .map(|value| normalize_path(Path::new(value), Path::new("/"))),
            source: row.get(3)?,
            started_at: row.get(4)?,
            ended_at: row.get(5)?,
            message_count: row.get(6)?,
            match_kind: None,
            preview: None,
        })
    })?;

    let mut sessions = Vec::new();
    for row in rows {
        sessions.push(row?);
    }
    Ok(sessions)
}

fn attach_cwd_matches(sessions: &mut [HermesSession], cwd: &Path) {
    for session in sessions {
        session.match_kind = session
            .cwd
            .as_deref()
            .and_then(|session_cwd| cwd_match_kind(session_cwd, cwd));
    }
}

fn cwd_match_kind(session_cwd: &Path, query_cwd: &Path) -> Option<CwdMatchKind> {
    if session_cwd == query_cwd {
        return Some(CwdMatchKind::Exact);
    }
    if query_cwd.starts_with(session_cwd) {
        return Some(CwdMatchKind::Ancestor);
    }
    if session_cwd.starts_with(query_cwd) {
        return Some(CwdMatchKind::Descendant);
    }
    None
}

fn sort_sessions(sessions: &mut [HermesSession]) {
    sessions.sort_by(|a, b| {
        let rank_a = a.match_kind.map(CwdMatchKind::rank).unwrap_or(2);
        let rank_b = b.match_kind.map(CwdMatchKind::rank).unwrap_or(2);
        rank_a
            .cmp(&rank_b)
            .then_with(|| b.updated_epoch().total_cmp(&a.updated_epoch()))
            .then_with(|| a.id.cmp(&b.id))
    });
}

pub(super) fn select_session(
    conn: &Connection,
    selector: &str,
    cwd: &Path,
    all: bool,
    db_path: &Path,
) -> Result<HermesSession> {
    let selector = selector.trim();
    if selector.is_empty() {
        anyhow::bail!("Hermes session selector must not be empty");
    }

    if selector.eq_ignore_ascii_case("latest") {
        return collect_sessions(conn, cwd, all, None, 1, db_path)?
            .into_iter()
            .next()
            .with_context(|| {
                if all {
                    format!("No Hermes sessions found in {}", db_path.display())
                } else {
                    format!("No Hermes sessions match cwd {}", cwd.display())
                }
            });
    }

    if let Some(index) = parse_session_index(selector)? {
        let mut sessions = collect_sessions(conn, cwd, all, None, usize::MAX, db_path)?;
        let total = sessions.len();
        if index > total {
            let scope = if all {
                format!("Hermes state DB {}", db_path.display())
            } else {
                format!("Hermes sessions matching cwd {}", cwd.display())
            };
            anyhow::bail!(
                "Hermes session index {index} is out of range; {scope} has {total} session(s) \
                 (indices are 1-based)"
            );
        }
        return Ok(sessions.remove(index - 1));
    }

    if let Some(session) = find_session_by_id(conn, selector)? {
        return Ok(session);
    }

    let mut title_matches = find_sessions_by_title(conn, selector)?;
    if title_matches.is_empty() {
        anyhow::bail!("No Hermes session found for id or title '{selector}'");
    }
    if !all {
        attach_cwd_matches(&mut title_matches, cwd);
        title_matches.retain(|session| session.match_kind.is_some());
        if title_matches.is_empty() {
            anyhow::bail!(
                "Hermes title '{selector}' exists, but none of its sessions match cwd {}. \
                 Pass --all or use --cwd for the owning project.",
                cwd.display()
            );
        }
    }
    sort_sessions(&mut title_matches);
    let best_rank = title_matches
        .first()
        .and_then(|session| session.match_kind.map(CwdMatchKind::rank))
        .unwrap_or(2);
    let same_best = title_matches
        .iter()
        .filter(|session| {
            session.match_kind.map(CwdMatchKind::rank).unwrap_or(2) == best_rank
                && (session.updated_epoch() - title_matches[0].updated_epoch()).abs() < f64::EPSILON
        })
        .count();
    if same_best > 1 {
        anyhow::bail!("{}", ambiguous_title_message(selector, &title_matches));
    }
    Ok(title_matches.remove(0))
}

fn parse_session_index(selector: &str) -> Result<Option<usize>> {
    if !selector.chars().all(|ch| ch.is_ascii_digit()) {
        return Ok(None);
    }

    let index = selector.parse::<usize>().with_context(|| {
        format!("Hermes session index '{selector}' is too large; indices are 1-based")
    })?;
    if index == 0 {
        anyhow::bail!(
            "Hermes session index 0 is invalid; indices are 1-based (use 1 for the first listed session)"
        );
    }
    Ok(Some(index))
}

fn find_session_by_id(conn: &Connection, id: &str) -> Result<Option<HermesSession>> {
    let mut sessions = load_sessions(conn)?;
    Ok(sessions.drain(..).find(|session| session.id == id))
}

fn find_sessions_by_title(conn: &Connection, title: &str) -> Result<Vec<HermesSession>> {
    Ok(load_sessions(conn)?
        .into_iter()
        .filter(|session| session.title.as_deref() == Some(title))
        .collect())
}

fn ambiguous_title_message(title: &str, sessions: &[HermesSession]) -> String {
    let mut message = format!(
        "Multiple Hermes sessions match title '{title}'. Use --session <id> to disambiguate:"
    );
    for session in sessions.iter().take(8) {
        let updated = session.updated_at().unwrap_or_else(|| "-".to_string());
        let cwd = session
            .cwd
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "-".to_string());
        message.push_str(&format!("\n  {}  {}  {}", session.id, updated, cwd));
    }
    message
}

fn search_session_preview(
    conn: &Connection,
    session_id: &str,
    terms: &[String],
    fts_available: bool,
) -> Result<Option<String>> {
    if fts_available {
        match search_session_preview_fts(conn, session_id, terms) {
            Ok(preview) => return Ok(preview),
            Err(err) => {
                tracing::debug!(session_id, error = %err, "Hermes FTS search failed; falling back to LIKE")
            }
        }
    }
    search_session_preview_like(conn, session_id, terms)
}

fn search_session_preview_fts(
    conn: &Connection,
    session_id: &str,
    terms: &[String],
) -> Result<Option<String>> {
    let fts_query = fts_exact_term(&terms[0]);
    let mut stmt = conn.prepare(
        "SELECT m.content \
         FROM messages_fts \
         JOIN messages m ON m.id = messages_fts.rowid \
         WHERE messages_fts MATCH ?1 \
           AND m.session_id = ?2 \
           AND COALESCE(m.active, 1) != 0 \
           AND LOWER(m.role) IN ('user', 'assistant', 'system') \
           AND m.content IS NOT NULL \
         ORDER BY m.timestamp ASC, m.id ASC \
         LIMIT 20",
    )?;
    let mut rows = stmt.query_map(params![fts_query, session_id], |row| {
        row.get::<_, String>(0)
    })?;
    let mut preview = None;
    if let Some(row) = rows.next() {
        let content = row?;
        preview = Some(snippet(&content, &terms[0], PREVIEW_WIDTH));
    }
    if preview.is_some() && allowed_transcript_has_all_terms(conn, session_id, terms)? {
        return Ok(preview);
    }
    Ok(None)
}

fn search_session_preview_like(
    conn: &Connection,
    session_id: &str,
    terms: &[String],
) -> Result<Option<String>> {
    let primary_pattern = format!("%{}%", escape_like(&terms[0].to_ascii_lowercase()));
    let mut stmt = conn.prepare(
        "SELECT content \
         FROM messages \
         WHERE session_id = ?1 \
           AND COALESCE(active, 1) != 0 \
           AND LOWER(role) IN ('user', 'assistant', 'system') \
           AND content IS NOT NULL \
           AND LOWER(content) LIKE ?2 ESCAPE '\\' \
         ORDER BY timestamp ASC, id ASC \
         LIMIT 100",
    )?;
    let mut rows = stmt.query_map(params![session_id, primary_pattern], |row| {
        row.get::<_, String>(0)
    })?;
    let mut preview = None;
    if let Some(row) = rows.next() {
        let content = row?;
        preview = Some(snippet(&content, &terms[0], PREVIEW_WIDTH));
    }
    if preview.is_some() && allowed_transcript_has_all_terms(conn, session_id, terms)? {
        return Ok(preview);
    }
    Ok(None)
}

pub(super) fn split_keyword_terms(keyword: &str) -> Result<Vec<String>> {
    let terms = keyword
        .split_whitespace()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if terms.is_empty() {
        anyhow::bail!("--keyword must contain at least one non-whitespace term");
    }
    Ok(terms)
}

fn allowed_transcript_has_all_terms(
    conn: &Connection,
    session_id: &str,
    terms: &[String],
) -> Result<bool> {
    let mut stmt = conn.prepare(
        "SELECT content \
         FROM messages \
         WHERE session_id = ?1 \
           AND COALESCE(active, 1) != 0 \
           AND LOWER(role) IN ('user', 'assistant', 'system') \
           AND content IS NOT NULL \
         ORDER BY timestamp ASC, id ASC",
    )?;
    let rows = stmt.query_map(params![session_id], |row| row.get::<_, String>(0))?;

    let mut present = vec![false; terms.len()];
    for row in rows {
        let content = row?;
        for (idx, term) in terms.iter().enumerate() {
            if !present[idx] && contains_ci(&content, term) {
                present[idx] = true;
            }
        }
        if present.iter().all(|found| *found) {
            return Ok(true);
        }
    }
    Ok(present.iter().all(|found| *found))
}

fn fts_exact_term(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

fn snippet(content: &str, keyword: &str, width: usize) -> String {
    let line = content
        .lines()
        .find(|line| contains_ci(line, keyword))
        .unwrap_or(content)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    truncate_display(&line, width)
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn sqlite_object_exists(conn: &Connection, name: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = ?1)",
        params![name],
        |row| row.get::<_, bool>(0),
    )
    .with_context(|| format!("Failed to inspect sqlite object '{name}'"))
}
