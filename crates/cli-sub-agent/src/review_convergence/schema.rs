use std::collections::HashSet;

use anyhow::{Context, Result};
use csa_process::ProviderTurnCompletion;
use csa_session::convergence::SemanticFindingIdentity;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum PageResponseStatus {
    Complete,
    Incomplete,
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
    response_status: PageResponseStatus,
    completion: ProviderTurnCompletion,
    candidate_limit: u32,
    candidate_count: u32,
    more_candidates_possible: bool,
    unscanned_items: Vec<String>,
    candidates: Vec<RawCandidate>,
}

pub(super) struct ParsedDiscoveryPage {
    pub(super) status: PageResponseStatus,
    pub(super) completion: ProviderTurnCompletion,
    pub(super) candidate_limit: u32,
    pub(super) more_candidates_possible: bool,
    pub(super) unscanned_items: Vec<String>,
    pub(super) candidates: Vec<SemanticFindingIdentity>,
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
    if page.candidate_limit == 0 {
        anyhow::bail!("candidate_limit must be greater than zero");
    }
    let actual_count = u32::try_from(page.candidates.len()).context("candidate count overflow")?;
    if page.candidate_count != actual_count {
        anyhow::bail!(
            "candidate_count {} does not match {} candidates",
            page.candidate_count,
            actual_count
        );
    }
    if page.candidate_count > page.candidate_limit {
        anyhow::bail!("candidate_count exceeds candidate_limit");
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
        completion: page.completion,
        candidate_limit: page.candidate_limit,
        more_candidates_possible: page.more_candidates_possible,
        unscanned_items: page.unscanned_items,
        candidates,
    })
}
