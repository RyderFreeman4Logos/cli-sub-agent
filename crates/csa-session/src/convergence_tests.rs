use chrono::{TimeZone, Utc};

use crate::convergence::{
    CampaignId, CampaignRecord, CoverageCellId, CoverageCellRecord, CoverageScope, EpochId,
    EpochRecord, GitObjectId, SemanticFindingIdentity, SemanticLens, Sha256Digest, StableFindingId,
};

fn digest(fill: char) -> Sha256Digest {
    Sha256Digest::parse(&format!("sha256:{}", fill.to_string().repeat(64)))
        .expect("test digest should be valid")
}

fn oid(fill: char) -> GitObjectId {
    GitObjectId::parse(&fill.to_string().repeat(40)).expect("test oid should be valid")
}

#[test]
fn campaign_id_rejects_invalid_ulid() {
    let generated = CampaignId::generate();
    assert_eq!(generated.as_str().len(), 26);

    let parsed = CampaignId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAV").expect("valid ULID");
    assert_eq!(parsed.as_str(), "01ARZ3NDEKTSV4RRFFQ69G5FAV");

    let error = CampaignId::parse("not-a-ulid").expect_err("invalid ULID must fail");
    assert!(error.to_string().contains("campaign id"));
    assert!(serde_json::from_str::<CampaignId>("\"not-a-ulid\"").is_err());
}

#[test]
fn digest_and_git_oid_canonicalize_and_reject_malformed_values() {
    let uppercase_digest = format!("sha256:{}", "AB".repeat(32));
    let digest = Sha256Digest::parse(&uppercase_digest).expect("uppercase hex is valid");
    assert_eq!(digest.as_str(), format!("sha256:{}", "ab".repeat(32)));
    assert_eq!(
        serde_json::to_string(&digest).unwrap(),
        format!("\"{}\"", digest)
    );

    let oid_40 = GitObjectId::parse(&"AB".repeat(20)).expect("SHA-1 oid");
    let oid_64 = GitObjectId::parse(&"CD".repeat(32)).expect("SHA-256 oid");
    assert_eq!(oid_40.as_str(), "ab".repeat(20));
    assert_eq!(oid_64.as_str(), "cd".repeat(32));

    for malformed in [
        "ab".repeat(19),
        "ab".repeat(21),
        format!("{}g", "a".repeat(39)),
    ] {
        assert!(
            GitObjectId::parse(&malformed).is_err(),
            "accepted {malformed}"
        );
    }
    for malformed in [
        "a".repeat(64),
        format!("sha256:{}", "a".repeat(63)),
        format!("sha256:{}g", "a".repeat(63)),
    ] {
        assert!(
            Sha256Digest::parse(&malformed).is_err(),
            "accepted {malformed}"
        );
    }
    assert!(serde_json::from_str::<GitObjectId>("\"xyz\"").is_err());
    assert!(serde_json::from_str::<Sha256Digest>("\"sha256:xyz\"").is_err());
}

#[test]
fn epoch_id_is_deterministic_and_record_validation_detects_tampering() {
    let base_oid = oid('a');
    let head_oid = oid('b');
    let diff_digest = digest('c');
    let first = EpochId::compute(&base_oid, &head_oid, &diff_digest);
    let second = EpochId::compute(&base_oid, &head_oid, &diff_digest);
    assert_eq!(first, second);

    let record = EpochRecord::new(base_oid, head_oid, diff_digest);
    assert_eq!(record.id(), &first);
    assert_eq!(record.recompute_id(), first);
    record.validate().expect("untampered epoch must validate");

    let mut encoded = serde_json::to_value(&record).unwrap();
    encoded["id"] = serde_json::Value::String(digest('d').to_string());
    let tampered: EpochRecord = serde_json::from_value(encoded).unwrap();
    let error = tampered.validate().expect_err("tampering must be detected");
    assert!(error.to_string().contains("epoch id mismatch"));
}

#[test]
fn coverage_cell_id_is_semantic_and_rejects_blank_scope_or_lens() {
    assert!(CoverageScope::new("  ", "crate:csa-session").is_err());
    assert!(CoverageScope::new("crate", "\n\t").is_err());
    assert!(SemanticLens::new("  ").is_err());

    let epoch = EpochId::compute(&oid('a'), &oid('b'), &digest('c'));
    let scope = CoverageScope::new(" crate ", " csa-session ").expect("valid scope");
    let lens = SemanticLens::new(" correctness ").expect("valid lens");
    assert_eq!(scope.kind(), "crate");
    assert_eq!(scope.key(), "csa-session");
    assert_eq!(lens.as_str(), "correctness");

    let first = CoverageCellId::compute(&epoch, &scope, &lens);
    let normalized_scope = CoverageScope::new("crate", "csa-session").unwrap();
    let normalized_lens = SemanticLens::new("correctness").unwrap();
    let second = CoverageCellId::compute(&epoch, &normalized_scope, &normalized_lens);
    assert_eq!(first, second);

    let record = CoverageCellRecord::new(epoch, scope, lens);
    assert_eq!(record.id(), &first);
    record.validate().expect("untampered cell must validate");

    let mut encoded = serde_json::to_value(&record).unwrap();
    encoded["id"] = serde_json::Value::String(digest('e').to_string());
    let tampered: CoverageCellRecord = serde_json::from_value(encoded).unwrap();
    assert!(tampered.validate().is_err());
}

#[test]
fn stable_finding_id_ignores_location_evidence_and_changes_with_semantics() {
    let identity = SemanticFindingIdentity::new(
        " missing cancellation guard ",
        " review worker lifecycle ",
        " resource leak ",
    )
    .expect("valid semantic identity");

    let location_before = ("src/review.rs", "40:1-52:2", "anchor-a");
    let location_after = ("src/worker.rs", "140:3-152:4", "anchor-b");
    assert_ne!(location_before, location_after);

    let before = StableFindingId::compute(&identity);
    let after = StableFindingId::compute(&identity);
    assert_eq!(before, after, "location evidence must not affect identity");

    let changed = SemanticFindingIdentity::new(
        "missing cancellation guard",
        "review worker lifecycle",
        "deadlock",
    )
    .unwrap();
    assert_ne!(before, StableFindingId::compute(&changed));

    assert!(SemanticFindingIdentity::new("", "component", "class").is_err());
    assert!(SemanticFindingIdentity::new("mechanism", " ", "class").is_err());
    assert!(SemanticFindingIdentity::new("mechanism", "component", "\n").is_err());
}

#[test]
fn schema_round_trip_rejects_unknown_fields() {
    let campaign = CampaignRecord::new(
        CampaignId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap(),
        Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap(),
        Some(digest('a')),
        Some(digest('b')),
    );
    let campaign_json = serde_json::to_string(&campaign).unwrap();
    assert_eq!(
        serde_json::from_str::<CampaignRecord>(&campaign_json).unwrap(),
        campaign
    );

    let mut unknown: serde_json::Value = serde_json::from_str(&campaign_json).unwrap();
    unknown["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<CampaignRecord>(unknown).is_err());

    let epoch = EpochRecord::new(oid('a'), oid('b'), digest('c'));
    let scope = CoverageScope::new("crate", "csa-session").unwrap();
    let lens = SemanticLens::new("correctness").unwrap();
    let cell = CoverageCellRecord::new(epoch.id().clone(), scope, lens);
    let cell_json = serde_json::to_string(&cell).unwrap();
    assert_eq!(
        serde_json::from_str::<CoverageCellRecord>(&cell_json).unwrap(),
        cell
    );

    let mut missing: serde_json::Value = serde_json::from_str(&cell_json).unwrap();
    missing.as_object_mut().unwrap().remove("lens");
    assert!(serde_json::from_value::<CoverageCellRecord>(missing).is_err());

    let identity = SemanticFindingIdentity::new("mechanism", "component", "class").unwrap();
    let mut identity_json = serde_json::to_value(&identity).unwrap();
    identity_json["path"] = serde_json::json!("src/lib.rs");
    assert!(serde_json::from_value::<SemanticFindingIdentity>(identity_json).is_err());
}
