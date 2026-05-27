//! Cross-session keyword search for `csa xurl recall --keyword`.
//!
//! Iterates every recall provider and reports threads whose transcript
//! matches the supplied keyword.  When `all = false`, results are filtered
//! to the current project.

use anyhow::Result;
use tracing::debug;

use super::{RECALL_PROVIDERS, provider_roots, thread_belongs_to_project, truncate_display};

struct Hit {
    provider: xurl_core::ProviderKind,
    thread_id: String,
    thread_source: String,
    updated_at: Option<String>,
    preview: Option<String>,
}

pub(super) fn handle_recall_keyword(keyword: &str, all: bool, limit: usize) -> Result<()> {
    let trimmed = keyword.trim();
    if trimmed.is_empty() {
        anyhow::bail!("--keyword must not be empty");
    }
    if limit == 0 {
        anyhow::bail!("--limit must be greater than 0");
    }

    let project_root = crate::pipeline::determine_project_root(None)?;
    let roots = provider_roots()?;
    let mut hits = collect_hits(trimmed, all, limit, &project_root, &roots);

    if hits.is_empty() {
        let scope = if all {
            "any project".to_string()
        } else {
            format!("project {}", project_root.display())
        };
        println!("No matches for keyword '{trimmed}' in {scope}.");
        return Ok(());
    }

    hits.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    print_hits(&hits);
    Ok(())
}

fn collect_hits(
    keyword: &str,
    all: bool,
    limit: usize,
    project_root: &std::path::Path,
    roots: &xurl_core::ProviderRoots,
) -> Vec<Hit> {
    let mut hits: Vec<Hit> = Vec::new();
    for &provider in RECALL_PROVIDERS {
        let query = xurl_core::ThreadQuery {
            uri: format!("{provider}://"),
            provider,
            role: None,
            q: Some(keyword.to_string()),
            limit,
            ignored_params: Vec::new(),
        };
        let result = match xurl_core::query_threads(&query, roots) {
            Ok(r) => r,
            Err(err) => {
                debug!(
                    provider = %provider,
                    error = %err,
                    "recall keyword: skipping provider"
                );
                continue;
            }
        };
        for item in result.items {
            if !all && !thread_belongs_to_project(&item.thread_source, project_root, provider) {
                continue;
            }
            hits.push(Hit {
                provider,
                thread_id: item.thread_id,
                thread_source: item.thread_source,
                updated_at: item.updated_at,
                preview: item.matched_preview,
            });
        }
    }
    hits
}

fn print_hits(hits: &[Hit]) {
    println!(
        "{:<10} {:<36} {:<20} PREVIEW",
        "PROVIDER", "SESSION", "UPDATED"
    );
    println!("{}", "-".repeat(100));
    for hit in hits {
        let updated = hit.updated_at.as_deref().unwrap_or("-");
        let updated_short: String = updated.chars().take(19).collect();
        let preview: String = hit
            .preview
            .as_deref()
            .unwrap_or("")
            .chars()
            .take(40)
            .collect();
        println!(
            "{:<10} {:<36} {:<20} {}",
            hit.provider.to_string(),
            truncate_display(&hit.thread_id, 36),
            updated_short,
            preview,
        );
        debug!(source = %hit.thread_source, "recall keyword hit");
    }
    println!("\nTotal matches: {}", hits.len());
}
