use std::collections::BTreeSet;

use super::*;

use csa_session::convergence::{EpochRecord, GitObjectId, Sha256Digest};

use super::super::coverage::{CoverageManifestPlan, plan_coverage_manifest};

fn epoch() -> EpochRecord {
    EpochRecord::new(
        GitObjectId::parse(BASE).expect("base OID should parse"),
        GitObjectId::parse(HEAD).expect("head OID should parse"),
        Sha256Digest::compute(b"coverage-manifest"),
    )
}

#[test]
fn coverage_manifest_order_and_cell_ids_are_deterministic_across_path_order() {
    let paths = vec![
        "crates/csa-session/src/convergence/ledger.rs".to_string(),
        "crates/cli-sub-agent/src/review_convergence/engine.rs".to_string(),
    ];
    let mut reversed = paths.clone();
    reversed.reverse();

    let first = plan_coverage_manifest(&epoch(), &paths).expect("manifest should plan");
    let second = plan_coverage_manifest(&epoch(), &reversed).expect("manifest should plan");
    let CoverageManifestPlan::Ready(first) = first else {
        panic!("two-path manifest must be bounded");
    };
    let CoverageManifestPlan::Ready(second) = second else {
        panic!("two-path manifest must be bounded");
    };
    let first_ids = first
        .cells()
        .iter()
        .map(|cell| cell.id().as_str().to_string())
        .collect::<Vec<_>>();
    let second_ids = second
        .cells()
        .iter()
        .map(|cell| cell.id().as_str().to_string())
        .collect::<Vec<_>>();
    assert_eq!(first_ids, second_ids);
    assert!(
        first
            .cells()
            .windows(2)
            .all(|pair| pair[0].id().as_str() < pair[1].id().as_str())
    );
    assert_eq!(
        first
            .cells()
            .iter()
            .map(|cell| cell.scope().kind())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["crate", "domain", "module"])
    );
}

#[test]
fn broad_changed_path_set_requires_structured_decomposition() {
    let paths = (0..9)
        .map(|index| format!("crates/domain-{index}/src/module-{index}/lib.rs"))
        .collect::<Vec<_>>();

    let plan = plan_coverage_manifest(&epoch(), &paths).expect("broad input must not panic");
    assert!(matches!(
        plan,
        CoverageManifestPlan::DecompositionRequired { .. }
    ));
}

#[tokio::test]
async fn broad_manifest_blocks_before_any_provider_call_instead_of_completing() {
    let mut broad_frozen = frozen();
    broad_frozen.changed_paths = (0..9)
        .map(|index| format!("crates/domain-{index}/src/module-{index}/lib.rs"))
        .collect();
    let mut probe = ScriptedProbe {
        captures: [Ok(broad_frozen)].into_iter().collect(),
        stable_fallback: None,
    };
    let mut runner = ScriptedRunner::default();
    let store = MemoryStore::default();

    let error = run_discovery_observation(&input(), &mut probe, &mut runner, &store)
        .await
        .expect_err("a broad manifest must require decomposition");

    assert_eq!(error.diagnostic().reason_code, "decomposition_required");
    assert_eq!(error.diagnostic().provider_calls, 0);
    assert!(runner.requests.is_empty());
}

#[tokio::test]
async fn every_manifest_cell_requires_its_own_saturation_page_before_completion() {
    let store = MemoryStore::default();
    let mut probe = ScriptedProbe::stable(19);
    let mut runner = ScriptedRunner::pages(
        std::iter::repeat_with(|| page("complete", 8, false, &[], Vec::new())).take(9),
    );

    let summary = run_discovery_observation(&input(), &mut probe, &mut runner, &store)
        .await
        .expect("all deterministic manifest cells should saturate");

    assert_eq!(summary.coverage_cell_count as usize, runner.requests.len());
    assert!(summary.coverage_cell_count > 1);
    assert_eq!(
        runner
            .requests
            .iter()
            .map(|request| request.cell.scope().kind())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["crate", "domain", "module"])
    );
}
