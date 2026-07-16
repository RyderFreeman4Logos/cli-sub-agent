use crate::convergence::{
    AdmittedModelIdentity, IndependentlyVerifiedModel, ModelEvidence, ModelEvidenceConfidence,
    ModelEvidenceProvenance, ObservedToolEvidence, ProviderTurnExecutionId, Sha256Digest,
};

fn admitted() -> AdmittedModelIdentity {
    AdmittedModelIdentity::new("codex", "openai", "gpt-5.6", "xhigh").expect("admitted")
}

fn observed() -> ObservedToolEvidence {
    ObservedToolEvidence::new("codex", "0.99.0").expect("observed")
}

#[test]
fn transport_report_is_not_an_actual_model_claim() {
    let evidence = ModelEvidence::host_observed(
        admitted(),
        observed(),
        Some("gpt-5.6-serving-alias"),
        ProviderTurnExecutionId::generate(),
    )
    .expect("model evidence");
    assert_eq!(
        evidence.provenance(),
        ModelEvidenceProvenance::TransportReported
    );
    assert_eq!(
        evidence.confidence(),
        ModelEvidenceConfidence::TransportReported
    );
    assert!(evidence.independently_verified_actual_model().is_none());
}

#[test]
fn verified_actual_model_requires_digest_bound_independent_proof() {
    let proof = IndependentlyVerifiedModel::new(
        admitted(),
        "provider-signed-attestation",
        Sha256Digest::compute(b"independent proof"),
    )
    .expect("proof");
    let evidence = ModelEvidence::host_observed(
        admitted(),
        observed(),
        None,
        ProviderTurnExecutionId::generate(),
    )
    .expect("model evidence")
    .with_independent_verification(proof);
    assert_eq!(
        evidence.provenance(),
        ModelEvidenceProvenance::IndependentlyVerified
    );
    assert_eq!(
        evidence.confidence(),
        ModelEvidenceConfidence::IndependentlyVerified
    );
    assert_eq!(
        evidence
            .independently_verified_actual_model()
            .expect("proof-backed actual model")
            .model()
            .model(),
        "gpt-5.6"
    );
}

#[test]
fn inconsistent_serialized_confidence_fails_closed() {
    let evidence = ModelEvidence::host_observed(
        admitted(),
        observed(),
        None,
        ProviderTurnExecutionId::generate(),
    )
    .expect("model evidence");
    let mut value = serde_json::to_value(evidence).expect("serialize");
    value["confidence"] = serde_json::json!("independently_verified");
    assert!(serde_json::from_value::<ModelEvidence>(value).is_err());
}
