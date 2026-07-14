pub(super) mod engine;
mod output;
pub(super) mod runner;
mod schema;

use std::path::Path;

use anyhow::Result;
use csa_session::convergence::{ConvergenceLedgerStore, Sha256Digest};

// Canonical JSON; object and cell keys are lexically ordered.
const POLICY_JSON: &[u8] = br#"{"coverage_cells":[{"lens":"broad_discovery","requirement":"required","scope":"whole_explicit_range"}],"kind":"convergence_discovery_observation","provider_call_budget_per_cell":4,"schema_version":1,"semantic_coverage":"walking_skeleton_not_exhaustive"}"#;

fn walking_skeleton_policy_digest() -> Sha256Digest {
    Sha256Digest::compute(POLICY_JSON)
}

pub(super) async fn run_command(
    project_root: &Path,
    range: &str,
    runner_context: runner::ProductionRunnerContext<'_>,
) -> Result<i32> {
    // This is a canonical serialization of the resolved, ordered catalog slice
    // the authoritative CSA adapter can actually admit for this command.
    let catalog_snapshot = serde_json::to_vec(&(
        runner_context.tier_name.as_deref(),
        runner_context.tier_model_spec.as_deref(),
        runner_context.tier_preference_order.as_slice(),
    ))?;
    let input = engine::ObservationInput::new(range, Sha256Digest::compute(&catalog_snapshot));
    let store = match ConvergenceLedgerStore::for_project(project_root) {
        Ok(store) => store,
        Err(error) => {
            eprintln!(
                "{}",
                serde_json::json!({
                    "kind": "convergence_discovery_blocked",
                    "reason_code": "store_failure",
                    "message": format!("{error:#}"),
                    "provider_calls": 0,
                    "discovery_evidence_complete": false,
                    "review_verdict": null,
                    "merge_attestation": false
                })
            );
            return Ok(1);
        }
    };
    let mut probe = runner::GitWorkspaceProbe::new(project_root);
    let mut discovery_runner = runner::ProductionDiscoveryRunner::new(runner_context);
    match engine::run_discovery_observation(&input, &mut probe, &mut discovery_runner, &store).await
    {
        Ok(summary) => {
            println!("{}", serde_json::to_string(&summary)?);
            Ok(0)
        }
        Err(error) => {
            eprintln!("{}", serde_json::to_string(error.diagnostic())?);
            Ok(1)
        }
    }
}

pub(super) async fn run_resolved_command(
    context: runner::ResolvedCommandContext<'_>,
) -> Result<i32> {
    let range = context
        .args
        .range
        .as_deref()
        .expect("validated convergence range")
        .to_owned();
    let project_root = context.project_root;
    let runner_context = context.runner_context();
    run_command(project_root, &range, runner_context).await
}

#[cfg(test)]
mod tests;
