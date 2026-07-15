use std::collections::HashSet;

use anyhow::{Context, Result};
use csa_session::convergence::SemanticFindingIdentity;
use serde::{Deserialize, Serialize};

const DISCOVERY_PAGE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PageKind {
    ConvergenceDiscoveryPage,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum PageResponseStatus {
    Complete,
    Partial,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCandidate {
    mechanism: String,
    affected_component: String,
    bug_class: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPage {
    schema_version: u32,
    kind: PageKind,
    response_status: PageResponseStatus,
    candidate_limit: u32,
    more_candidates_possible: bool,
    unscanned_items: Vec<String>,
    candidates: Vec<RawCandidate>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct ParsedDiscoveryPage {
    pub(super) status: PageResponseStatus,
    pub(super) candidate_limit: u32,
    pub(super) more_candidates_possible: bool,
    pub(super) unscanned_items: Vec<String>,
    pub(super) candidates: Vec<SemanticFindingIdentity>,
}

impl ParsedDiscoveryPage {
    pub(super) fn continuation_required(&self) -> bool {
        self.status == PageResponseStatus::Partial
            || self.more_candidates_possible
            || !self.unscanned_items.is_empty()
            || u32::try_from(self.candidates.len()).ok() == Some(self.candidate_limit)
    }
}

pub(super) fn parse_discovery_page(raw: &str) -> Result<ParsedDiscoveryPage> {
    let json = if raw.starts_with("```json\n") {
        let body = raw
            .strip_prefix("```json\n")
            .context("missing JSON fence opener")?;
        body.strip_suffix("\n```\n")
            .or_else(|| body.strip_suffix("\n```"))
            .context("response must contain one complete JSON fence with no trailing prose")?
    } else {
        raw
    };
    let mut deserializer = serde_json::Deserializer::from_str(json);
    let page = RawPage::deserialize(&mut deserializer).context("invalid discovery page JSON")?;
    deserializer
        .end()
        .context("discovery page contains trailing content")?;
    if page.schema_version != DISCOVERY_PAGE_SCHEMA_VERSION {
        anyhow::bail!(
            "unsupported discovery page schema version {}",
            page.schema_version
        );
    }
    if !matches!(page.kind, PageKind::ConvergenceDiscoveryPage) {
        anyhow::bail!("unexpected discovery page kind");
    }
    if page.candidate_limit == 0 {
        anyhow::bail!("candidate_limit must be greater than zero");
    }
    let actual_count = u32::try_from(page.candidates.len()).context("candidate count overflow")?;
    if actual_count > page.candidate_limit {
        anyhow::bail!("candidate array exceeds candidate_limit");
    }
    let continuation_signalled = page.more_candidates_possible || !page.unscanned_items.is_empty();
    match page.response_status {
        PageResponseStatus::Complete if continuation_signalled => {
            anyhow::bail!("complete page must not carry continuation signals");
        }
        PageResponseStatus::Partial if !continuation_signalled => {
            anyhow::bail!("partial page must carry an explicit continuation signal");
        }
        PageResponseStatus::Complete | PageResponseStatus::Partial => {}
    }
    let mut unscanned = HashSet::new();
    for item in &page.unscanned_items {
        let normalized = item.trim();
        if normalized.is_empty() {
            anyhow::bail!("unscanned_items must not contain blank entries");
        }
        if !unscanned.insert(normalized) {
            anyhow::bail!("unscanned_items must not contain duplicates");
        }
    }
    let mut fingerprints = HashSet::new();
    let mut candidates = Vec::with_capacity(page.candidates.len());
    for raw_candidate in page.candidates {
        let identity = SemanticFindingIdentity::new(
            &raw_candidate.mechanism,
            &raw_candidate.affected_component,
            &raw_candidate.bug_class,
        )?;
        let fingerprint = csa_session::convergence::StableFindingId::compute(&identity);
        if !fingerprints.insert(fingerprint.as_str().to_string()) {
            anyhow::bail!("duplicate semantic fingerprint in discovery page");
        }
        candidates.push(identity);
    }
    Ok(ParsedDiscoveryPage {
        status: page.response_status,
        candidate_limit: page.candidate_limit,
        more_candidates_possible: page.more_candidates_possible,
        unscanned_items: page.unscanned_items,
        candidates,
    })
}
