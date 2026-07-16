use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};

use csa_session::convergence::{
    AdmittedModelIdentity, ArtifactEvidenceRef, CampaignId, CsaSessionId, EpochRecord, GitObjectId,
    ModelEvidence, ObservedToolEvidence, ProviderTurnExecutionId, SessionRelativeArtifactPath,
    Sha256Digest,
};

use super::clean_room_v2::{
    HostReviewArtifactStore, ReviewEnvelopeContext,
    parse_legacy_v1_clean_room_review_for_read_only, parse_provider_clean_room_response,
};

fn epoch(head: u8) -> EpochRecord {
    EpochRecord::new(
        GitObjectId::parse(&"11".repeat(20)).expect("base"),
        GitObjectId::parse(&format!("{head:02x}").repeat(20)).expect("head"),
        Sha256Digest::compute(&[head]),
    )
}

fn artifact(label: &[u8]) -> ArtifactEvidenceRef {
    ArtifactEvidenceRef::new(
        CsaSessionId::generate(),
        SessionRelativeArtifactPath::new("output/gates.json").expect("path"),
        Sha256Digest::compute(label),
    )
}

fn context(gate: ArtifactEvidenceRef) -> ReviewEnvelopeContext {
    let admitted =
        AdmittedModelIdentity::new("codex", "openai", "gpt-5.6", "xhigh").expect("admitted model");
    let observed = ObservedToolEvidence::new("codex", "0.99.0").expect("observed tool");
    let evidence = ModelEvidence::host_observed(
        admitted,
        observed,
        Some("gpt-5.6"),
        ProviderTurnExecutionId::generate(),
    )
    .expect("model evidence");
    ReviewEnvelopeContext::new(CampaignId::generate(), epoch(2), gate, evidence)
}

fn provider_review() -> String {
    serde_json::json!({
        "findings": [{
            "semantic_identity": {
                "violated_invariant": "published review evidence remains host authoritative",
                "trigger_failure_mode": "provider attempts to select an artifact",
                "primary_component": "clean room envelope",
                "bug_class": "authority boundary"
            },
            "review_text": "The host must bind the terminal evidence."
        }],
        "questions": [],
        "unchecked_items": [],
        "review_text": "One bounded finding was observed."
    })
    .to_string()
}

#[test]
fn parser_accepts_only_bounded_provider_review_data() {
    let review = parse_provider_clean_room_response(&provider_review()).expect("provider review");
    assert_eq!(review.findings.len(), 1);
    assert_eq!(review.questions, Vec::<String>::new());
    assert_eq!(review.unchecked_items, Vec::<String>::new());
}

#[test]
fn parser_rejects_giant_duplicate_controls_and_authority_fields() {
    let giant = format!(
        "{{\"findings\":[],\"questions\":[],\"unchecked_items\":[],\"review_text\":\"{}\"}}",
        "x".repeat(50 * 1024)
    );
    assert!(parse_provider_clean_room_response(&giant).is_err());

    for raw in [
        r#"{"findings":[],"questions":[],"unchecked_items":[],"review_text":"one","review_text":"two"}"#,
        r#"{"findings":[],"questions":[],"unchecked_items":[],"review_text":"\u001b[31mred"}"#,
        r#"{"findings":[],"questions":[],"unchecked_items":[],"review_text":"x","artifact":{}}"#,
        r#"{"findings":[],"questions":[],"unchecked_items":[],"review_text":"x","model_identity":{}}"#,
        r#"{"findings":[],"questions":[],"unchecked_items":[],"review_text":"x","path":"/tmp/x"}"#,
        r#"{"findings":[],"questions":[],"unchecked_items":[],"review_text":"x","digest":"sha256:x"}"#,
        r#"{"findings":[],"questions":[],"unchecked_items":[],"review_text":"x","schema_version":2}"#,
        r#"{"findings":[],"questions":[],"unchecked_items":[],"review_text":"x","response_status":"incomplete"}"#,
        r#"provider prose before {"#,
    ] {
        assert!(
            parse_provider_clean_room_response(raw).is_err(),
            "accepted {raw}"
        );
    }

    let nested_path = r#"{"findings":[{"semantic_identity":{"violated_invariant":"a","trigger_failure_mode":"b","primary_component":"c","bug_class":"d"},"review_text":"x","path":"src/lib.rs"}],"questions":[],"unchecked_items":[],"review_text":"x"}"#;
    assert!(parse_provider_clean_room_response(nested_path).is_err());
}

#[test]
fn host_envelope_round_trip_redacts_secrets_and_is_private() {
    let directory = tempfile::tempdir().expect("temporary output directory");
    let store = HostReviewArtifactStore::new(
        directory.path(),
        CsaSessionId::generate(),
        SessionRelativeArtifactPath::new("output").expect("relative output"),
    )
    .expect("host store");
    let context = context(artifact(b"gate"));
    let raw = r#"{"findings":[],"questions":[],"unchecked_items":[],"review_text":"api_key=sk-supersecret98765 Bearer anothersecret98765"}"#;
    let output = store.publish(&context, raw).expect("published v2 review");
    assert!(output.review_text().contains("[REDACTED]"));
    assert!(!output.review_text().contains("supersecret"));
    let reread = store
        .readback(&context, output.artifact())
        .expect("readback");
    assert_eq!(reread, output);

    let path = directory.path().join(
        output
            .artifact()
            .path()
            .as_str()
            .strip_prefix("output/")
            .expect("output-relative artifact"),
    );
    assert_eq!(
        fs::metadata(path)
            .expect("artifact metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn envelope_rejects_epoch_gate_and_digest_substitution() {
    let directory = tempfile::tempdir().expect("temporary output directory");
    let store = HostReviewArtifactStore::new(
        directory.path(),
        CsaSessionId::generate(),
        SessionRelativeArtifactPath::new("output").expect("relative output"),
    )
    .expect("host store");
    let context = context(artifact(b"gate one"));
    let output = store
        .publish(&context, &provider_review())
        .expect("published v2 review");

    let wrong_epoch = ReviewEnvelopeContext::new(
        CampaignId::generate(),
        epoch(3),
        artifact(b"gate one"),
        output.model_evidence().clone(),
    );
    assert!(store.readback(&wrong_epoch, output.artifact()).is_err());

    let wrong_gate = ReviewEnvelopeContext::new(
        CampaignId::generate(),
        epoch(2),
        artifact(b"other gate"),
        output.model_evidence().clone(),
    );
    assert!(store.readback(&wrong_gate, output.artifact()).is_err());

    let path = directory.path().join(
        output
            .artifact()
            .path()
            .as_str()
            .strip_prefix("output/")
            .expect("output-relative artifact"),
    );
    fs::write(path, b"tampered").expect("tamper artifact fixture");
    assert!(store.readback(&context, output.artifact()).is_err());
}

#[test]
fn readback_rejects_inode_replacement_and_never_reuses_old_review_evidence() {
    let directory = tempfile::tempdir().expect("temporary output directory");
    let store = HostReviewArtifactStore::new(
        directory.path(),
        CsaSessionId::generate(),
        SessionRelativeArtifactPath::new("output").expect("relative output"),
    )
    .expect("host store");
    let context = context(artifact(b"gate"));
    let output = store
        .publish(&context, &provider_review())
        .expect("published review artifact");
    let path = directory.path().join(
        output
            .artifact()
            .path()
            .as_str()
            .strip_prefix("output/")
            .expect("output-relative artifact"),
    );
    let replacement = directory.path().join("replacement-review.json");
    fs::write(&replacement, b"old evidence").expect("replacement fixture");
    fs::remove_file(&path).expect("remove original artifact");
    symlink(&replacement, &path).expect("replace artifact path with symlink");

    assert!(store.readback(&context, output.artifact()).is_err());
}

#[test]
fn legacy_v1_reader_is_inspection_only() {
    let legacy = serde_json::json!({
        "schema_version": 1,
        "kind": "convergence_clean_room_review",
        "artifact": artifact(b"legacy"),
        "model_identity": {
            "tool": "codex", "provider": "openai", "model": "gpt-5.4", "reasoning": "high"
        },
        "findings": [],
        "questions": [],
        "unchecked_items": []
    })
    .to_string();
    assert!(parse_legacy_v1_clean_room_review_for_read_only(&legacy).is_ok());
    assert!(parse_provider_clean_room_response(&legacy).is_err());
}
