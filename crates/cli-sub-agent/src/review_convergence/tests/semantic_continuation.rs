use super::*;

use csa_session::convergence::{SemanticFindingIdentity, StableFindingId};

use super::super::continuation::{ContinuationEvidence, ContinuationFinding};

#[test]
fn continuation_prompt_carries_semantic_identities_unscanned_and_uncovered_evidence() {
    let mut request = DiscoveryRequest::for_test(frozen());
    request.intent = DiscoveryRunIntent::Continuation;
    let identity = SemanticFindingIdentity::new(
        "discovery attempts must only finalize after every page candidate is durable",
        "provider reports candidates before finalization",
        "review_convergence::engine::run_discovery_observation",
        "atomicity",
    )
    .expect("semantic finding identity should be valid");
    let stable_finding_id = StableFindingId::compute(&identity);
    request.continuation = ContinuationEvidence::new(
        vec![ContinuationFinding::new(
            stable_finding_id.clone(),
            identity.clone(),
        )],
        vec!["cross-domain authorization checks".to_string()],
        vec![request.cell.clone()],
        vec!["uncovered manifest item: credential rotation path".to_string()],
    );

    let prompt = super::super::runner::build_discovery_prompt(&request);

    assert!(prompt.contains(stable_finding_id.as_str()));
    assert!(prompt.contains(identity.violated_invariant()));
    assert!(prompt.contains(identity.trigger_failure_mode()));
    assert!(prompt.contains(identity.primary_component()));
    assert!(prompt.contains(identity.bug_class()));
    assert!(prompt.contains("cross-domain authorization checks"));
    assert!(prompt.contains(request.cell.id().as_str()));
    assert!(prompt.contains("credential rotation path"));
    assert!(!prompt.contains("Existing semantic fingerprints"));
}

#[tokio::test]
async fn continuation_request_reconstructs_semantic_evidence_from_the_finalized_ledger_page() {
    let store = MemoryStore::default();
    let mut probe = ScriptedProbe::stable(5);
    let mut runner = ScriptedRunner::pages([
        page(
            "partial",
            8,
            true,
            &["cross-domain authorization checks"],
            vec![candidate("missing authorization handoff")],
        ),
        page("complete", 8, false, &[], Vec::new()),
    ]);

    run_discovery_observation(&input(), &mut probe, &mut runner, &store)
        .await
        .expect("continuation evidence should complete after a saturation page");

    let continuation = &runner.requests[1].continuation;
    assert_eq!(runner.requests[1].intent, DiscoveryRunIntent::Continuation);
    assert_eq!(continuation.findings.len(), 1);
    assert_eq!(
        continuation.latest_finalized_unscanned_items,
        ["cross-domain authorization checks"]
    );
    assert!(continuation.uncovered_cells.len() > 1);
    assert!(
        continuation
            .uncovered_cells
            .contains(&runner.requests[1].cell),
        "the partial page's cell remains uncovered alongside the other manifest cells"
    );
    assert_eq!(
        continuation.uncovered_items,
        ["cross-domain authorization checks"]
    );
}
