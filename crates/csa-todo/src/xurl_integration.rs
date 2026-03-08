//! xurl-core integration for TODO plan context enrichment.
//!
//! Wraps xurl-core's thread query and resolution API behind a simplified
//! interface suitable for CSA TODO workflows (e.g., importing transcripts
//! as reference files).

use anyhow::{Context, Result};

/// Summary of a conversation thread discovered via xurl-core.
#[derive(Debug, Clone)]
pub struct ThreadSummary {
    /// Provider-specific session identifier.
    pub id: String,
    /// Provider name (claude, codex, gemini, etc.).
    pub provider: String,
    /// Human-readable title extracted from metadata, if available.
    pub title: Option<String>,
    /// ISO-8601 creation timestamp, if available.
    pub created_at: Option<String>,
}

/// List conversation threads matching an optional keyword filter.
///
/// Delegates to `xurl_core::query_threads` with provider roots resolved from
/// the current environment. Returns at most `limit` results (default: 20).
pub fn list_threads(keyword: Option<&str>, limit: Option<usize>) -> Result<Vec<ThreadSummary>> {
    use xurl_core::{ProviderKind, ProviderRoots, ThreadQuery};

    let roots = ProviderRoots::from_env_or_home().context("Failed to resolve provider roots")?;
    let effective_limit = limit.unwrap_or(20);

    let providers = [
        ProviderKind::Claude,
        ProviderKind::Codex,
        ProviderKind::Gemini,
        ProviderKind::Opencode,
    ];

    let mut summaries = Vec::new();

    for &provider in &providers {
        let query = ThreadQuery {
            uri: format!("agents://{provider}"),
            provider,
            role: None,
            q: keyword.map(|s| s.to_string()),
            limit: effective_limit,
            ignored_params: Vec::new(),
        };

        match xurl_core::query_threads(&query, &roots) {
            Ok(result) => {
                for item in result.items {
                    summaries.push(ThreadSummary {
                        id: item.thread_id.clone(),
                        provider: format!("{provider}"),
                        title: item.matched_preview.clone(),
                        created_at: item.updated_at.clone(),
                    });
                }
            }
            Err(e) => {
                // Provider directory may not exist — log and continue
                tracing::debug!(
                    provider = %provider,
                    error = %e,
                    "Skipping provider during thread listing"
                );
            }
        }
    }

    // Truncate to requested limit across all providers
    summaries.truncate(effective_limit);

    Ok(summaries)
}

/// Import a conversation transcript as markdown.
///
/// Resolves the thread identified by `provider` + `session_id` via xurl-core,
/// then renders it as markdown suitable for storage as a TODO reference file.
pub fn import_transcript(provider: &str, session_id: &str) -> Result<String> {
    use xurl_core::ProviderRoots;

    let roots = ProviderRoots::from_env_or_home().context("Failed to resolve provider roots")?;

    let uri_str = format!("agents://{provider}/{session_id}");
    let uri: xurl_core::AgentsUri = uri_str
        .parse()
        .with_context(|| format!("Invalid agents URI: {uri_str}"))?;

    let resolved = xurl_core::resolve_thread(&uri, &roots)
        .with_context(|| format!("Failed to resolve thread {uri_str}"))?;

    let markdown = xurl_core::render_thread_markdown(&uri, &resolved)
        .with_context(|| format!("Failed to render thread {uri_str}"))?;

    Ok(markdown)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thread_summary_construction() {
        let summary = ThreadSummary {
            id: "abc-123".to_string(),
            provider: "claude".to_string(),
            title: Some("Test conversation".to_string()),
            created_at: Some("2026-03-01T00:00:00Z".to_string()),
        };

        assert_eq!(summary.id, "abc-123");
        assert_eq!(summary.provider, "claude");
        assert!(summary.title.is_some());
        assert!(summary.created_at.is_some());
    }

    #[test]
    fn test_thread_summary_without_optional_fields() {
        let summary = ThreadSummary {
            id: "def-456".to_string(),
            provider: "gemini".to_string(),
            title: None,
            created_at: None,
        };

        assert_eq!(summary.provider, "gemini");
        assert!(summary.title.is_none());
        assert!(summary.created_at.is_none());
    }

    #[test]
    fn test_xurl_core_types_accessible() {
        // Compilation test: verify xurl-core types are usable from this crate
        let _provider = xurl_core::ProviderKind::Claude;
        let _provider2 = xurl_core::ProviderKind::Codex;
        let _provider3 = xurl_core::ProviderKind::Gemini;
    }
}
