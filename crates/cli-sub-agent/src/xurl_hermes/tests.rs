use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};

use super::db::{collect_sessions, open_state_db, select_session};
use super::render::{render_session_markdown, threads_json};
use super::{CwdMatchKind, full_session_output_guard_message};

fn create_fixture(with_fts: bool) -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("state.db");
    let conn = Connection::open(&db_path).expect("open fixture");
    conn.execute_batch(
        "
        CREATE TABLE sessions (
            id TEXT PRIMARY KEY,
            source TEXT NOT NULL,
            started_at REAL NOT NULL,
            ended_at REAL,
            cwd TEXT,
            title TEXT,
            message_count INTEGER DEFAULT 0,
            archived INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            role TEXT NOT NULL,
            content TEXT,
            timestamp REAL NOT NULL,
            active INTEGER NOT NULL DEFAULT 1
        );
        ",
    )
    .expect("schema");
    if with_fts {
        conn.execute_batch(
            "
            CREATE VIRTUAL TABLE messages_fts USING fts5(content);
            CREATE TRIGGER messages_fts_insert AFTER INSERT ON messages BEGIN
                INSERT INTO messages_fts(rowid, content) VALUES (new.id, COALESCE(new.content, ''));
            END;
            ",
        )
        .expect("fts");
    }
    drop(conn);
    (temp, db_path)
}

fn insert_session(
    conn: &Connection,
    id: &str,
    title: &str,
    cwd: &str,
    started_at: f64,
    ended_at: Option<f64>,
) {
    conn.execute(
        "INSERT INTO sessions (id, source, started_at, ended_at, cwd, title, message_count) \
         VALUES (?1, 'fixture', ?2, ?3, ?4, ?5, 2)",
        params![id, started_at, ended_at, cwd, title],
    )
    .expect("insert session");
}

fn insert_message(conn: &Connection, session_id: &str, role: &str, content: &str, timestamp: f64) {
    conn.execute(
        "INSERT INTO messages (session_id, role, content, timestamp) VALUES (?1, ?2, ?3, ?4)",
        params![session_id, role, content, timestamp],
    )
    .expect("insert message");
}

#[test]
fn cwd_matching_orders_exact_before_descendant_even_when_older() {
    let (_temp, db_path) = create_fixture(false);
    let conn = Connection::open(&db_path).expect("open");
    insert_session(&conn, "exact-old", "Exact", "/repo", 10.0, Some(20.0));
    insert_session(&conn, "child-new", "Child", "/repo/crate", 30.0, Some(40.0));
    insert_session(
        &conn,
        "other-newest",
        "Other",
        "/repo-other",
        50.0,
        Some(60.0),
    );

    let sessions =
        collect_sessions(&conn, Path::new("/repo"), false, None, 10, &db_path).expect("collect");

    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].id, "exact-old");
    assert_eq!(sessions[0].match_kind, Some(CwdMatchKind::Exact));
    assert_eq!(sessions[1].id, "child-new");
    assert_eq!(sessions[1].match_kind, Some(CwdMatchKind::Descendant));
}

#[test]
fn title_and_session_id_selection_work() {
    let (_temp, db_path) = create_fixture(false);
    let conn = Connection::open(&db_path).expect("open");
    insert_session(&conn, "sid-1", "Readable title", "/repo", 10.0, Some(20.0));

    let by_id =
        select_session(&conn, "sid-1", Path::new("/repo"), false, &db_path).expect("select by id");
    let by_title = select_session(&conn, "Readable title", Path::new("/repo"), false, &db_path)
        .expect("select by title");

    assert_eq!(by_id.id, "sid-1");
    assert_eq!(by_title.id, "sid-1");
}

#[test]
fn numeric_session_index_selects_first_listed_session() {
    let (_temp, db_path) = create_fixture(false);
    let conn = Connection::open(&db_path).expect("open");
    insert_session(&conn, "older-exact", "Older", "/repo", 10.0, Some(20.0));
    insert_session(&conn, "newer-exact", "Newer", "/repo", 30.0, Some(40.0));
    insert_session(
        &conn,
        "newer-child",
        "Child",
        "/repo/crate",
        50.0,
        Some(60.0),
    );

    let listed =
        collect_sessions(&conn, Path::new("/repo"), false, None, 10, &db_path).expect("collect");
    let selected =
        select_session(&conn, "1", Path::new("/repo"), false, &db_path).expect("select index");

    assert_eq!(listed[0].id, "newer-exact");
    assert_eq!(selected.id, listed[0].id);
}

#[test]
fn out_of_range_session_index_reports_available_count() {
    let (_temp, db_path) = create_fixture(false);
    let conn = Connection::open(&db_path).expect("open");
    insert_session(&conn, "sid-only", "Only", "/repo", 10.0, Some(20.0));

    let err = select_session(&conn, "2", Path::new("/repo"), false, &db_path)
        .expect_err("out-of-range index must fail");
    let msg = err.to_string();

    assert!(
        msg.contains("Hermes session index 2 is out of range"),
        "{msg}"
    );
    assert!(msg.contains("has 1 session(s)"), "{msg}");
    assert!(msg.contains("indices are 1-based"), "{msg}");
}

#[test]
fn zero_session_index_is_rejected_as_invalid() {
    let (_temp, db_path) = create_fixture(false);
    let conn = Connection::open(&db_path).expect("open");
    insert_session(&conn, "sid-only", "Only", "/repo", 10.0, Some(20.0));

    let err = select_session(&conn, "0", Path::new("/repo"), false, &db_path)
        .expect_err("zero index must fail");
    let msg = err.to_string();

    assert!(msg.contains("Hermes session index 0 is invalid"), "{msg}");
    assert!(msg.contains("indices are 1-based"), "{msg}");
}

#[test]
fn session_index_takes_precedence_over_numeric_id_or_title() {
    let (_temp, db_path) = create_fixture(false);
    let conn = Connection::open(&db_path).expect("open");
    insert_session(&conn, "1", "1", "/repo", 10.0, Some(20.0));
    insert_session(&conn, "sid-first", "First", "/repo", 30.0, Some(40.0));

    let selected =
        select_session(&conn, "1", Path::new("/repo"), false, &db_path).expect("select index");

    assert_eq!(selected.id, "sid-first");
}

#[test]
fn duplicate_title_reports_actionable_disambiguation() {
    let (_temp, db_path) = create_fixture(false);
    let conn = Connection::open(&db_path).expect("open");
    insert_session(&conn, "sid-a", "Repeated", "/repo", 10.0, Some(20.0));
    insert_session(&conn, "sid-b", "Repeated", "/repo", 10.0, Some(20.0));

    let err = select_session(&conn, "Repeated", Path::new("/repo"), false, &db_path)
        .expect_err("duplicate title must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("Multiple Hermes sessions match title"),
        "{msg}"
    );
    assert!(msg.contains("sid-a"), "{msg}");
    assert!(msg.contains("sid-b"), "{msg}");
    assert!(msg.contains("--session <id>"), "{msg}");
}

#[test]
fn keyword_search_falls_back_to_like_without_fts() {
    let (_temp, db_path) = create_fixture(false);
    let conn = Connection::open(&db_path).expect("open");
    insert_session(&conn, "sid-1", "Searchable", "/repo", 10.0, Some(20.0));
    insert_message(
        &conn,
        "sid-1",
        "user",
        "please inspect the review finding in this repo",
        11.0,
    );

    let sessions = collect_sessions(
        &conn,
        Path::new("/repo"),
        false,
        Some("review finding"),
        10,
        &db_path,
    )
    .expect("search");

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "sid-1");
    assert!(
        sessions[0]
            .preview
            .as_deref()
            .unwrap_or_default()
            .contains("review finding")
    );
}

#[test]
fn keyword_search_like_matches_terms_across_allowed_messages() {
    let (_temp, db_path) = create_fixture(false);
    let conn = Connection::open(&db_path).expect("open");
    insert_session(&conn, "sid-like", "Searchable", "/repo", 10.0, Some(20.0));
    insert_message(&conn, "sid-like", "user", "foo appears first", 11.0);
    insert_message(&conn, "sid-like", "assistant", "bar appears later", 12.0);

    let sessions = collect_sessions(
        &conn,
        Path::new("/repo"),
        false,
        Some("foo bar"),
        10,
        &db_path,
    )
    .expect("search");

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "sid-like");
    let preview = sessions[0].preview.as_deref().unwrap_or_default();
    assert!(preview.contains("foo appears first"), "{preview}");
}

#[test]
fn keyword_search_like_rejects_terms_present_only_in_tool_messages() {
    let (_temp, db_path) = create_fixture(false);
    let conn = Connection::open(&db_path).expect("open");
    insert_session(&conn, "sid-like", "Searchable", "/repo", 10.0, Some(20.0));
    insert_message(&conn, "sid-like", "user", "foo appears first", 11.0);
    insert_message(&conn, "sid-like", "tool", "bar is hidden tool output", 12.0);

    let sessions = collect_sessions(
        &conn,
        Path::new("/repo"),
        false,
        Some("foo bar"),
        10,
        &db_path,
    )
    .expect("search");

    assert!(sessions.is_empty(), "{sessions:?}");
}

#[test]
fn keyword_search_like_omits_tool_message_preview() {
    let (_temp, db_path) = create_fixture(false);
    let conn = Connection::open(&db_path).expect("open");
    insert_session(&conn, "sid-like", "Searchable", "/repo", 10.0, Some(20.0));
    insert_message(
        &conn,
        "sid-like",
        "tool",
        "needlelike SYNTHETIC_TOOL_SECRET_LIKE",
        11.0,
    );
    insert_message(
        &conn,
        "sid-like",
        "user",
        "needlelike visible user preview",
        12.0,
    );

    let sessions = collect_sessions(
        &conn,
        Path::new("/repo"),
        false,
        Some("needlelike"),
        10,
        &db_path,
    )
    .expect("search");

    assert_eq!(sessions.len(), 1);
    let preview = sessions[0].preview.as_deref().unwrap_or_default();
    assert!(preview.contains("visible user preview"), "{preview}");
    assert!(!preview.contains("SYNTHETIC_TOOL_SECRET_LIKE"), "{preview}");
}

#[test]
fn keyword_search_fts_matches_terms_across_allowed_messages() {
    let (_temp, db_path) = create_fixture(true);
    let conn = Connection::open(&db_path).expect("open");
    insert_session(&conn, "sid-fts", "Searchable", "/repo", 10.0, Some(20.0));
    insert_message(&conn, "sid-fts", "user", "foo appears first", 11.0);
    insert_message(&conn, "sid-fts", "assistant", "bar appears later", 12.0);

    let sessions = collect_sessions(
        &conn,
        Path::new("/repo"),
        false,
        Some("foo bar"),
        10,
        &db_path,
    )
    .expect("search");

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "sid-fts");
    let preview = sessions[0].preview.as_deref().unwrap_or_default();
    assert!(preview.contains("foo appears first"), "{preview}");
}

#[test]
fn keyword_search_fts_rejects_terms_present_only_in_tool_messages() {
    let (_temp, db_path) = create_fixture(true);
    let conn = Connection::open(&db_path).expect("open");
    insert_session(&conn, "sid-fts", "Searchable", "/repo", 10.0, Some(20.0));
    insert_message(&conn, "sid-fts", "user", "foo appears first", 11.0);
    insert_message(&conn, "sid-fts", "tool", "bar is hidden tool output", 12.0);

    let sessions = collect_sessions(
        &conn,
        Path::new("/repo"),
        false,
        Some("foo bar"),
        10,
        &db_path,
    )
    .expect("search");

    assert!(sessions.is_empty(), "{sessions:?}");
}

#[test]
fn keyword_search_fts_omits_tool_message_preview() {
    let (_temp, db_path) = create_fixture(true);
    let conn = Connection::open(&db_path).expect("open");
    insert_session(&conn, "sid-fts", "Searchable", "/repo", 10.0, Some(20.0));
    insert_message(
        &conn,
        "sid-fts",
        "tool",
        "needlefts SYNTHETIC_TOOL_SECRET_FTS",
        11.0,
    );
    insert_message(
        &conn,
        "sid-fts",
        "assistant",
        "needlefts visible assistant preview",
        12.0,
    );

    let sessions = collect_sessions(
        &conn,
        Path::new("/repo"),
        false,
        Some("needlefts"),
        10,
        &db_path,
    )
    .expect("search");

    assert_eq!(sessions.len(), 1);
    let preview = sessions[0].preview.as_deref().unwrap_or_default();
    assert!(preview.contains("visible assistant preview"), "{preview}");
    assert!(!preview.contains("SYNTHETIC_TOOL_SECRET_FTS"), "{preview}");
}

#[test]
fn full_session_guard_blocks_large_terminal_output_without_page() {
    let markdown = "x".repeat(crate::recall_cmd::OUTPUT_GUARD_BYTES);
    let message =
        full_session_output_guard_message("sid-large", &markdown, true).expect("guard message");

    assert!(message.contains("OUTPUT_TOO_LARGE"), "{message}");
    assert!(
        message.contains("csa xurl recall --provider hermes --session sid-large | tail -100"),
        "{message}"
    );
}

#[test]
fn full_session_guard_allows_large_non_terminal_output() {
    let markdown = "x".repeat(crate::recall_cmd::OUTPUT_GUARD_BYTES);

    assert!(full_session_output_guard_message("sid-large", &markdown, false).is_none());
}

#[test]
fn threads_json_contains_hermes_fields() {
    let (_temp, db_path) = create_fixture(false);
    let conn = Connection::open(&db_path).expect("open");
    insert_session(&conn, "sid-json", "JSON Title", "/repo", 10.0, Some(20.0));

    let sessions =
        collect_sessions(&conn, Path::new("/repo"), false, None, 10, &db_path).expect("collect");
    let json = threads_json(&sessions, &db_path).expect("json");
    let value: serde_json::Value = serde_json::from_str(&json).expect("parse json");

    assert_eq!(value[0]["provider"], "hermes");
    assert_eq!(value[0]["thread_id"], "sid-json");
    assert_eq!(value[0]["title"], "JSON Title");
    assert_eq!(value[0]["cwd"], "/repo");
    assert_eq!(value[0]["cwd_match"], "exact");
}

#[test]
fn render_session_markdown_omits_tool_messages() {
    let (_temp, db_path) = create_fixture(false);
    let conn = Connection::open(&db_path).expect("open");
    insert_session(&conn, "sid-render", "Render", "/repo", 10.0, Some(20.0));
    insert_message(&conn, "sid-render", "user", "hello", 11.0);
    insert_message(
        &conn,
        "sid-render",
        "tool",
        "{\"secret\":\"tool dump\"}",
        12.0,
    );
    insert_message(&conn, "sid-render", "assistant", "hi", 13.0);

    let session =
        select_session(&conn, "sid-render", Path::new("/repo"), false, &db_path).expect("select");
    let markdown = render_session_markdown(&conn, &session, &db_path).expect("render");

    assert!(markdown.contains("## 1. User"));
    assert!(markdown.contains("hello"));
    assert!(markdown.contains("## 2. Assistant"));
    assert!(markdown.contains("hi"));
    assert!(!markdown.contains("tool dump"));
}

#[test]
fn missing_state_db_reports_path() {
    let temp = tempfile::tempdir().expect("tempdir");
    let missing = temp.path().join("state.db");
    let err = open_state_db(&missing).expect_err("missing db must fail");
    let msg = err.to_string();
    assert!(msg.contains("Hermes state DB not found"), "{msg}");
    assert!(msg.contains(&missing.display().to_string()), "{msg}");
}

#[cfg(unix)]
#[test]
fn no_permission_state_db_reports_open_error() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let (_temp, db_path) = create_fixture(false);
    let original = fs::metadata(&db_path).expect("metadata").permissions();
    fs::set_permissions(&db_path, fs::Permissions::from_mode(0o000)).expect("chmod unreadable");

    let err = open_state_db(&db_path).expect_err("unreadable db must fail");
    fs::set_permissions(&db_path, original).expect("restore permissions");

    let msg = format!("{err:#}");
    assert!(
        msg.contains("Failed to open Hermes state DB read-only"),
        "{msg}"
    );
}
