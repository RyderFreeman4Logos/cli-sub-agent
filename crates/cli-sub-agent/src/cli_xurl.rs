// NOTE #1858: #[path]-included by tests; no `crate::`, no binary-only methods (dead_code).
#![allow(dead_code)]
//! CLI subcommand for xurl thread queries.

use std::path::PathBuf;

use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum XurlCommands {
    /// List available conversation threads across AI tool providers
    Threads {
        /// Filter threads by keyword
        #[arg(long)]
        keyword: Option<String>,

        /// Filter by provider (amp, codex, claude, gemini, pi, opencode, hermes)
        #[arg(long)]
        provider: Option<String>,

        /// Working directory used by the Hermes provider (defaults to process cwd).
        #[arg(long, value_name = "PATH")]
        cwd: Option<PathBuf>,

        /// Hermes home directory used by the Hermes provider (defaults to HERMES_HOME or ~/.hermes).
        #[arg(long, value_name = "PATH")]
        hermes_home: Option<PathBuf>,

        /// Hermes profile used by the Hermes provider.
        #[arg(long)]
        hermes_profile: Option<String>,

        /// Maximum results per provider
        #[arg(long, default_value = "20")]
        limit: usize,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Recover main-agent context from recorded session transcripts.
    ///
    /// Modes (default is "read most recent session"):
    /// * `--keyword "A B C"` — search every recorded session; whitespace splits
    ///   the value into terms that must ALL appear (AND logic, not exact phrase).
    /// * `--session ULID` — render a specific session transcript as markdown.
    /// * `--keyword TEXT --session ULID` — search within the specified session
    ///   for the keyword(s).
    /// * `--page N` — render compact page N of the latest session
    ///   (page 0 = current, higher = older).
    Recall {
        /// Search recorded sessions for these terms (whitespace-separated, AND).
        ///
        /// Without `--session`, scans every recorded session; with `--session`,
        /// restricts the search to that session's transcript.
        #[arg(long, conflicts_with_all = ["page", "list"])]
        keyword: Option<String>,

        /// Render this session as markdown (ULID, history index, or `latest`).
        ///
        /// Combined with `--keyword`, restricts the keyword search to this
        /// session instead of scanning all recorded sessions.
        #[arg(long, conflicts_with = "list")]
        session: Option<String>,

        /// Render compact page N of the selected or latest session (newest-first).
        /// Page 0 = current page, 1 = previous, etc.
        #[arg(long, conflicts_with_all = ["keyword", "list"])]
        page: Option<u32>,

        /// List recorded sessions instead of reading or searching.
        #[arg(long, conflicts_with_all = ["keyword", "session", "page"])]
        list: bool,

        /// With `--list` / `--keyword`: include all projects (not just the current one).
        #[arg(long)]
        all: bool,

        /// With `--list` / `--keyword`: cap how many results to return.
        #[arg(long, default_value = "10")]
        limit: usize,

        /// Provider-specific recall backend. Currently supports `hermes` and `codex`.
        #[arg(long)]
        provider: Option<String>,

        /// Working directory used by the Hermes provider (defaults to process cwd).
        #[arg(long, value_name = "PATH")]
        cwd: Option<PathBuf>,

        /// Hermes home directory used by the Hermes provider (defaults to HERMES_HOME or ~/.hermes).
        #[arg(long, value_name = "PATH")]
        hermes_home: Option<PathBuf>,

        /// Hermes profile used by the Hermes provider.
        #[arg(long)]
        hermes_profile: Option<String>,
    },
}

const ALL_PROVIDERS: &[xurl_core::ProviderKind] = &[
    xurl_core::ProviderKind::Claude,
    xurl_core::ProviderKind::Codex,
    xurl_core::ProviderKind::Gemini,
    xurl_core::ProviderKind::Amp,
    xurl_core::ProviderKind::Opencode,
    xurl_core::ProviderKind::Pi,
];

pub(crate) fn parse_provider(s: &str) -> Result<xurl_core::ProviderKind> {
    match s.to_ascii_lowercase().as_str() {
        "amp" => Ok(xurl_core::ProviderKind::Amp),
        "codex" => Ok(xurl_core::ProviderKind::Codex),
        "claude" => Ok(xurl_core::ProviderKind::Claude),
        "gemini" => Ok(xurl_core::ProviderKind::Gemini),
        "pi" => Ok(xurl_core::ProviderKind::Pi),
        "opencode" => Ok(xurl_core::ProviderKind::Opencode),
        _ => anyhow::bail!(
            "Unknown provider: '{s}'. Valid: amp, codex, claude, gemini, pi, opencode, hermes"
        ),
    }
}

pub fn handle_threads(
    keyword: Option<String>,
    provider: Option<String>,
    limit: usize,
    json: bool,
) -> Result<()> {
    let roots = xurl_core::ProviderRoots::from_env_or_home()
        .map_err(|e| anyhow::anyhow!("Failed to resolve provider roots: {e}"))?;

    let providers: Vec<xurl_core::ProviderKind> = if let Some(ref p) = provider {
        vec![parse_provider(p)?]
    } else {
        ALL_PROVIDERS.to_vec()
    };

    let mut all_items: Vec<serde_json::Value> = Vec::new();

    for prov in &providers {
        let query = xurl_core::ThreadQuery {
            uri: format!("{prov}://"),
            provider: *prov,
            role: None,
            q: keyword.clone(),
            limit,
            ignored_params: Vec::new(),
        };

        match xurl_core::query_threads(&query, &roots) {
            Ok(result) => {
                for item in &result.items {
                    all_items.push(serde_json::json!({
                        "thread_id": item.thread_id,
                        "provider": prov.to_string(),
                        "uri": item.uri,
                        "source": item.thread_source,
                        "updated_at": item.updated_at,
                        "preview": item.matched_preview,
                    }));
                }
            }
            Err(e) => {
                tracing::debug!(provider = %prov, error = %e, "skipping provider");
            }
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&all_items)?);
    } else if all_items.is_empty() {
        println!("No threads found.");
    } else {
        // Table output
        println!(
            "{:<12} {:<40} {:<20} PREVIEW",
            "PROVIDER", "THREAD_ID", "UPDATED"
        );
        println!("{}", "-".repeat(100));
        for item in &all_items {
            let provider = item["provider"].as_str().unwrap_or("-");
            let thread_id = item["thread_id"].as_str().unwrap_or("-");
            // Truncate thread_id to 38 chars for table display
            let thread_id_display = if thread_id.len() > 38 {
                &thread_id[..38]
            } else {
                thread_id
            };
            let updated = item["updated_at"].as_str().unwrap_or("-");
            // Truncate updated to 18 chars
            let updated_display = if updated.len() > 18 {
                &updated[..18]
            } else {
                updated
            };
            let preview = item["preview"]
                .as_str()
                .unwrap_or("")
                .chars()
                .take(40)
                .collect::<String>();
            println!("{provider:<12} {thread_id_display:<40} {updated_display:<20} {preview}");
        }
        println!("\nTotal: {} thread(s)", all_items.len());
    }

    Ok(())
}
