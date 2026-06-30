//! xurl command dispatcher.
//!
//! `cli_xurl.rs` only defines the `XurlCommands` enum and parsing helpers; it
//! is `#[path]`-included from integration tests (via `cli.rs`) that don't have
//! every binary-side module in scope. The recall dispatch lives here so it can
//! reach `crate::recall_cmd::*` without forcing those tests to declare every
//! transitively referenced module.
use anyhow::Result;
use csa_core::types::OutputFormat;

use crate::cli::XurlCommands;
use crate::recall_cmd;

#[path = "xurl_hermes.rs"]
mod hermes;

struct ThreadDispatch {
    keyword: Option<String>,
    provider: Option<String>,
    cwd: Option<std::path::PathBuf>,
    hermes_home: Option<std::path::PathBuf>,
    hermes_profile: Option<String>,
    limit: usize,
    json: bool,
}

struct RecallDispatch {
    keyword: Option<String>,
    session: Option<String>,
    page: Option<u32>,
    list: bool,
    all: bool,
    limit: usize,
    provider: Option<String>,
    cwd: Option<std::path::PathBuf>,
    hermes_home: Option<std::path::PathBuf>,
    hermes_profile: Option<String>,
}

pub fn handle_xurl(cmd: XurlCommands, output_format: OutputFormat) -> Result<()> {
    match cmd {
        XurlCommands::Threads {
            keyword,
            provider,
            cwd,
            hermes_home,
            hermes_profile,
            limit,
            json,
        } => handle_threads(ThreadDispatch {
            keyword,
            provider,
            cwd,
            hermes_home,
            hermes_profile,
            limit,
            json: json || matches!(output_format, OutputFormat::Json),
        }),
        XurlCommands::Recall {
            keyword,
            session,
            page,
            list,
            all,
            limit,
            provider,
            cwd,
            hermes_home,
            hermes_profile,
        } => handle_recall(RecallDispatch {
            keyword,
            session,
            page,
            list,
            all,
            limit,
            provider,
            cwd,
            hermes_home,
            hermes_profile,
        }),
    }
}

fn handle_threads(args: ThreadDispatch) -> Result<()> {
    let ThreadDispatch {
        keyword,
        provider,
        cwd,
        hermes_home,
        hermes_profile,
        limit,
        json,
    } = args;

    if provider
        .as_deref()
        .is_some_and(|p| p.eq_ignore_ascii_case("hermes"))
    {
        return hermes::handle_threads(hermes::HermesThreadArgs {
            keyword,
            cwd,
            hermes_home,
            hermes_profile,
            limit,
            json,
        });
    }
    if cwd.is_some() || hermes_home.is_some() || hermes_profile.is_some() {
        anyhow::bail!("--cwd/--hermes-home/--hermes-profile require --provider hermes");
    }
    crate::cli::handle_threads(keyword, provider, limit, json)
}

fn handle_recall(args: RecallDispatch) -> Result<()> {
    let RecallDispatch {
        keyword,
        session,
        page,
        list,
        all,
        limit,
        provider,
        cwd,
        hermes_home,
        hermes_profile,
    } = args;

    if provider
        .as_deref()
        .is_some_and(|p| p.eq_ignore_ascii_case("hermes"))
    {
        return hermes::handle_recall(hermes::HermesRecallArgs {
            keyword,
            session,
            page,
            list,
            all,
            limit,
            cwd,
            hermes_home,
            hermes_profile,
        });
    }
    if cwd.is_some() || hermes_home.is_some() || hermes_profile.is_some() {
        anyhow::bail!("--cwd/--hermes-home/--hermes-profile require --provider hermes");
    }

    let provider = provider
        .as_deref()
        .map(crate::cli::parse_provider)
        .transpose()?;
    if let Some(provider) = provider {
        if provider != xurl_core::ProviderKind::Codex {
            anyhow::bail!(
                "csa xurl recall --provider currently supports 'hermes' and 'codex'; got '{provider}'"
            );
        }
        if list {
            return recall_cmd::handle_recall_list_for_provider_cmd(provider, limit, all);
        }
        if let Some(kw) = keyword {
            return recall_cmd::handle_recall_keyword_for_provider(
                provider,
                &kw,
                session.as_deref(),
                all,
                limit,
            );
        }
        let sid = session.as_deref().unwrap_or("latest");
        return recall_cmd::handle_recall_read_for_provider_cmd(provider, sid, page);
    }

    if list {
        return recall_cmd::handle_recall_list_cmd(limit, all);
    }
    if let Some(kw) = keyword {
        return recall_cmd::handle_recall_keyword(&kw, session.as_deref(), all, limit);
    }
    if let Some(sid) = session {
        return recall_cmd::handle_recall_read_cmd(&sid, page);
    }
    recall_cmd::handle_recall_read_cmd("latest", page)
}
