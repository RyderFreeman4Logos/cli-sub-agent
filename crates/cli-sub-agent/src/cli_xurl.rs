#![allow(dead_code)]
//! CLI subcommand for xurl thread queries.

use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum XurlCommands {
    /// List available conversation threads across AI tool providers
    Threads {
        /// Filter threads by keyword
        #[arg(long)]
        keyword: Option<String>,

        /// Filter by provider (amp, codex, claude, gemini, pi, opencode)
        #[arg(long)]
        provider: Option<String>,

        /// Maximum results per provider
        #[arg(long, default_value = "20")]
        limit: usize,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

pub fn handle_xurl(cmd: XurlCommands) -> Result<()> {
    match cmd {
        XurlCommands::Threads {
            keyword,
            provider,
            limit,
            json,
        } => handle_threads(keyword, provider, limit, json),
    }
}

const ALL_PROVIDERS: &[xurl_core::ProviderKind] = &[
    xurl_core::ProviderKind::Claude,
    xurl_core::ProviderKind::Codex,
    xurl_core::ProviderKind::Gemini,
    xurl_core::ProviderKind::Amp,
    xurl_core::ProviderKind::Opencode,
    xurl_core::ProviderKind::Pi,
];

fn parse_provider(s: &str) -> Result<xurl_core::ProviderKind> {
    match s.to_ascii_lowercase().as_str() {
        "amp" => Ok(xurl_core::ProviderKind::Amp),
        "codex" => Ok(xurl_core::ProviderKind::Codex),
        "claude" => Ok(xurl_core::ProviderKind::Claude),
        "gemini" => Ok(xurl_core::ProviderKind::Gemini),
        "pi" => Ok(xurl_core::ProviderKind::Pi),
        "opencode" => Ok(xurl_core::ProviderKind::Opencode),
        _ => anyhow::bail!(
            "Unknown provider: '{s}'. Valid: amp, codex, claude, gemini, pi, opencode"
        ),
    }
}

fn handle_threads(
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
            println!(
                "{:<12} {:<40} {:<20} {}",
                provider, thread_id_display, updated_display, preview
            );
        }
        println!("\nTotal: {} thread(s)", all_items.len());
    }

    Ok(())
}
