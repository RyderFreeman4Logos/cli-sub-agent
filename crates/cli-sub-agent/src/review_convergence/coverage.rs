use std::collections::BTreeSet;

use anyhow::{Result, bail};
use csa_process::ProviderTurnCompletion;
use csa_session::convergence::{
    CampaignRecord, ConvergenceEvent, ConvergenceLedger, CoverageCellRecord, CoverageScope,
    EpochRecord, SemanticLens,
};

const MAX_MANIFEST_SCOPES: usize = 8;
const MANIFEST_LENSES: [&str; 3] = ["correctness", "security", "resource_lifecycle"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CoverageManifest {
    cells: Vec<CoverageCellRecord>,
}

impl CoverageManifest {
    pub(super) fn cells(&self) -> &[CoverageCellRecord] {
        &self.cells
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CoverageManifestPlan {
    Ready(CoverageManifest),
    DecompositionRequired {
        scope_count: usize,
        maximum_scope_count: usize,
    },
}

pub(super) fn plan_coverage_manifest(
    epoch: &EpochRecord,
    changed_paths: &[String],
) -> Result<CoverageManifestPlan> {
    let scopes = manifest_scopes(changed_paths)?;
    if scopes.len() > MAX_MANIFEST_SCOPES {
        return Ok(CoverageManifestPlan::DecompositionRequired {
            scope_count: scopes.len(),
            maximum_scope_count: MAX_MANIFEST_SCOPES,
        });
    }
    let mut cells = Vec::with_capacity(scopes.len() * MANIFEST_LENSES.len());
    for (kind, key) in scopes {
        for lens in MANIFEST_LENSES {
            cells.push(CoverageCellRecord::new(
                epoch.id().clone(),
                CoverageScope::new(&kind, &key)?,
                SemanticLens::new(lens)?,
            ));
        }
    }
    cells.sort_by(|left, right| left.id().as_str().cmp(right.id().as_str()));
    Ok(CoverageManifestPlan::Ready(CoverageManifest { cells }))
}

pub(super) fn uncovered_manifest_cells(
    ledger: &ConvergenceLedger,
    campaign: &CampaignRecord,
    manifest: &CoverageManifest,
) -> Vec<CoverageCellRecord> {
    let finalized_attempts = ledger
        .entries()
        .iter()
        .filter(|entry| entry.campaign_id() == campaign.id())
        .filter_map(|entry| match entry.event() {
            ConvergenceEvent::DiscoveryAttemptFinalized(record) => {
                Some(record.discovery_attempt_id().as_str())
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    manifest
        .cells()
        .iter()
        .filter(|cell| {
            !ledger
                .entries()
                .iter()
                .rev()
                .filter(|entry| entry.campaign_id() == campaign.id())
                .filter_map(|entry| match entry.event() {
                    ConvergenceEvent::DiscoveryAttemptRecorded(record)
                        if record.coverage_cell_id() == cell.id()
                            && finalized_attempts.contains(record.id().as_str()) =>
                    {
                        Some(record)
                    }
                    _ => None,
                })
                .any(is_saturated_attempt)
        })
        .cloned()
        .collect()
}

fn manifest_scopes(changed_paths: &[String]) -> Result<BTreeSet<(String, String)>> {
    if changed_paths.is_empty() {
        return Ok(BTreeSet::from([
            ("crate".to_string(), "workspace".to_string()),
            ("domain".to_string(), "cross_cutting".to_string()),
            ("module".to_string(), "root".to_string()),
        ]));
    }
    let mut scopes = BTreeSet::new();
    for path in changed_paths {
        let components = normalized_path_components(path)?;
        let crate_key = if components[0] == "crates" && components.len() >= 2 {
            format!("crates/{}", components[1])
        } else {
            components[0].to_string()
        };
        let module_key = if components.len() == 1 {
            "root".to_string()
        } else {
            components[..components.len() - 1].join("/")
        };
        let domain_key = components
            .iter()
            .position(|component| *component == "src")
            .and_then(|index| components.get(index + 1))
            .map_or_else(
                || crate_key.clone(),
                |domain| format!("{crate_key}::{domain}"),
            );
        scopes.insert(("crate".to_string(), crate_key));
        scopes.insert(("domain".to_string(), domain_key));
        scopes.insert(("module".to_string(), module_key));
    }
    Ok(scopes)
}

fn normalized_path_components(path: &str) -> Result<Vec<&str>> {
    if path.is_empty() || path.starts_with('/') || path.contains('\0') {
        bail!("changed path must be a non-empty relative path");
    }
    let components = path.split('/').collect::<Vec<_>>();
    if components
        .iter()
        .any(|component| component.is_empty() || *component == "." || *component == "..")
    {
        bail!("changed path must not contain empty, dot, or parent components");
    }
    Ok(components)
}

fn is_saturated_attempt(attempt: &csa_session::convergence::DiscoveryAttemptRecord) -> bool {
    attempt.completion() == ProviderTurnCompletion::Natural
        && attempt.reported_candidate_count() == 0
        && !attempt.more_candidates_possible()
        && attempt.unscanned_items().is_empty()
}
