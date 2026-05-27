//! xurl command dispatcher.
//!
//! `cli_xurl.rs` only defines the `XurlCommands` enum and parsing helpers; it
//! is `#[path]`-included from integration tests (via `cli.rs`) that don't have
//! every binary-side module in scope. The recall dispatch lives here so it can
//! reach `crate::recall_cmd::*` without forcing those tests to declare every
//! transitively referenced module.
use anyhow::Result;

use crate::cli::XurlCommands;
use crate::recall_cmd;

pub fn handle_xurl(cmd: XurlCommands) -> Result<()> {
    match cmd {
        XurlCommands::Threads {
            keyword,
            provider,
            limit,
            json,
        } => crate::cli::handle_threads(keyword, provider, limit, json),
        XurlCommands::Recall {
            keyword,
            session,
            page,
            list,
            all,
            limit,
        } => handle_recall(keyword, session, page, list, all, limit),
    }
}

fn handle_recall(
    keyword: Option<String>,
    session: Option<String>,
    page: Option<u32>,
    list: bool,
    all: bool,
    limit: usize,
) -> Result<()> {
    if list {
        return recall_cmd::handle_recall_list_cmd(limit, all);
    }
    if let Some(kw) = keyword {
        return recall_cmd::handle_recall_keyword(&kw, all, limit);
    }
    if let Some(sid) = session {
        return recall_cmd::handle_recall_read_cmd(&sid, page);
    }
    recall_cmd::handle_recall_read_cmd("latest", page)
}
