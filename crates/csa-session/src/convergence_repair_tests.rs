use crate::convergence::{
    CandidateId, EpochRecord, GitObjectId, RepairBatchRecord, RootClusterRecord, Sha256Digest,
};

fn digest(fill: char) -> Sha256Digest {
    Sha256Digest::parse(&format!("sha256:{}", fill.to_string().repeat(64)))
        .expect("test digest should be valid")
}

fn epoch() -> EpochRecord {
    EpochRecord::new(
        GitObjectId::parse(&"a".repeat(40)).expect("test base oid"),
        GitObjectId::parse(&"b".repeat(40)).expect("test head oid"),
        digest('c'),
    )
}

fn candidate(value: &str) -> CandidateId {
    CandidateId::parse(value).expect("test candidate id")
}

#[test]
fn repair_records_digest_complete_sets_canonically() {
    let epoch = epoch();
    let candidates = vec![
        candidate("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
        candidate("01ARZ3NDEKTSV4RRFFQ69G5FAW"),
    ];
    let cluster = RootClusterRecord::new(
        epoch.id().clone(),
        "every repair handoff must be complete",
        candidates.clone(),
        digest('d'),
    )
    .expect("cluster should be valid");
    let same_cluster = RootClusterRecord::new(
        epoch.id().clone(),
        "every repair handoff must be complete",
        candidates.clone(),
        digest('d'),
    )
    .expect("cluster should be stable");
    assert_eq!(
        cluster.candidate_set_digest(),
        same_cluster.candidate_set_digest()
    );

    let batch = RepairBatchRecord::new(
        cluster.id().clone(),
        epoch.id().clone(),
        candidates,
        digest('d'),
        vec!["validate the immutable handoff".to_string()],
        vec!["exercise a changed candidate union".to_string()],
        vec!["document repair authorization".to_string()],
        vec!["preserve current ledger readers".to_string()],
        vec!["audit sibling repair launches".to_string()],
    )
    .expect("batch should be valid");
    assert_ne!(cluster.candidate_set_digest(), batch.content_digest());

    assert!(
        RootClusterRecord::new(
            epoch.id().clone(),
            "every repair handoff must be complete",
            vec![candidate("01ARZ3NDEKTSV4RRFFQ69G5FAW")],
            digest('d'),
        )
        .expect("changed member should be valid")
        .candidate_set_digest()
            != cluster.candidate_set_digest()
    );
}
