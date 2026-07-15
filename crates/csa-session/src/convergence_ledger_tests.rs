use chrono::{DateTime, TimeZone, Utc};
use serde_json::{Value, json};

use crate::convergence::{
    CONVERGENCE_LEDGER_SCHEMA_VERSION, CampaignId, CampaignRecord, ConvergenceEvent,
    ConvergenceLedger, ConvergenceLedgerEntry, CoverageCellRecord, CoverageScope, EpochRecord,
    GitObjectId, LedgerEventId, SemanticLens, Sha256Digest,
};

const CAMPAIGN_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const CAMPAIGN_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const EVENT_1: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const EVENT_2: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAY";
const EVENT_3: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAZ";
const EVENT_4: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB0";
const EVENT_5: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB1";
const EVENT_6: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB2";

fn at(second: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, second).unwrap()
}

fn campaign(value: &str) -> CampaignId {
    CampaignId::parse(value).unwrap()
}

fn event_id(value: &str) -> LedgerEventId {
    LedgerEventId::parse(value).unwrap()
}

fn digest(fill: char) -> Sha256Digest {
    Sha256Digest::parse(&format!("sha256:{}", fill.to_string().repeat(64))).unwrap()
}

fn oid(fill: char) -> GitObjectId {
    GitObjectId::parse(&fill.to_string().repeat(40)).unwrap()
}

fn campaign_record(id: &CampaignId) -> CampaignRecord {
    CampaignRecord::for_test(id.clone(), at(0), Some(digest('d')))
}

fn epoch_record() -> EpochRecord {
    EpochRecord::new(oid('a'), oid('b'), digest('c'))
}

fn cell_record(epoch: &EpochRecord) -> CoverageCellRecord {
    CoverageCellRecord::new(
        epoch.id().clone(),
        CoverageScope::new("crate", "csa-session").unwrap(),
        SemanticLens::new("correctness").unwrap(),
    )
}

fn entry(
    sequence: u64,
    event_id_value: &str,
    campaign_id: &CampaignId,
    event: ConvergenceEvent,
) -> ConvergenceLedgerEntry {
    ConvergenceLedgerEntry::new(
        sequence,
        event_id(event_id_value),
        campaign_id.clone(),
        at(u32::try_from(sequence).unwrap()),
        event,
    )
}

fn ledger(entries: Vec<ConvergenceLedgerEntry>) -> ConvergenceLedger {
    serde_json::from_value(json!({
        "schema_version": CONVERGENCE_LEDGER_SCHEMA_VERSION,
        "entries": entries,
    }))
    .unwrap()
}

#[test]
fn convergence_ledger_accepts_valid_campaign_epoch_cell_history() {
    let campaign_id = campaign(CAMPAIGN_A);
    let campaign_record = campaign_record(&campaign_id);
    let epoch = epoch_record();
    let cell = cell_record(&epoch);
    let ledger = ledger(vec![
        entry(
            1,
            EVENT_1,
            &campaign_id,
            ConvergenceEvent::CampaignStarted(campaign_record),
        ),
        entry(
            2,
            EVENT_2,
            &campaign_id,
            ConvergenceEvent::EpochOpened(epoch),
        ),
        entry(
            3,
            EVENT_3,
            &campaign_id,
            ConvergenceEvent::CoverageCellDefined(cell),
        ),
    ]);

    ledger.validate().expect("valid history must pass");
    assert_eq!(ledger.schema_version(), 1);
    assert_eq!(ledger.entries().len(), 3);
    assert_eq!(ledger.entries()[0].sequence(), 1);
    assert_eq!(ledger.entries()[0].event_id().as_str(), EVENT_1);
    assert_eq!(ledger.entries()[0].campaign_id(), &campaign_id);
    assert_eq!(ledger.entries()[0].recorded_at(), &at(1));
    assert!(matches!(
        ledger.entries()[2].event(),
        ConvergenceEvent::CoverageCellDefined(_)
    ));

    let empty = ConvergenceLedger::default();
    assert_eq!(empty, ConvergenceLedger::empty());
    assert_eq!(empty.schema_version(), CONVERGENCE_LEDGER_SCHEMA_VERSION);
    assert!(empty.entries().is_empty());
    empty.validate().expect("empty v1 ledger must be valid");

    let generated = LedgerEventId::generate();
    assert_eq!(generated.as_str().len(), 26);
    assert_eq!(LedgerEventId::parse(EVENT_1).unwrap().as_str(), EVENT_1);
    assert!(LedgerEventId::parse("not-a-ulid").is_err());
    assert!(serde_json::from_str::<LedgerEventId>("42").is_err());
}

#[test]
fn convergence_ledger_append_batch_validates_the_full_batch_before_publication() {
    let campaign_id = campaign(CAMPAIGN_A);
    let epoch = epoch_record();
    let cell = cell_record(&epoch);
    let mut ledger = ConvergenceLedger::empty();

    let error = ledger
        .append_batch(
            campaign_id.clone(),
            vec![
                ConvergenceEvent::CampaignStarted(campaign_record(&campaign_id)),
                ConvergenceEvent::EpochOpened(epoch.clone()),
                ConvergenceEvent::CoverageCellDefined(cell.clone()),
                ConvergenceEvent::CoverageCellDefined(cell),
            ],
        )
        .expect_err("the duplicate final event makes the complete batch invalid");

    assert!(error.to_string().contains("coverage cell"));
    assert_eq!(ledger, ConvergenceLedger::empty());

    ledger
        .append_batch(
            campaign_id.clone(),
            vec![
                ConvergenceEvent::CampaignStarted(campaign_record(&campaign_id)),
                ConvergenceEvent::EpochOpened(epoch.clone()),
                ConvergenceEvent::CoverageCellDefined(cell_record(&epoch)),
            ],
        )
        .expect("a valid batch must become visible together");
    assert_eq!(ledger.entries().len(), 3);
    assert_eq!(ledger.entries()[0].sequence(), 1);
    assert_eq!(ledger.entries()[2].sequence(), 3);
    ledger.validate().expect("published batch must be valid");
}

#[test]
fn convergence_ledger_rejects_noncontiguous_sequence_and_duplicate_event_ids() {
    let campaign_id = campaign(CAMPAIGN_A);
    let noncontiguous = ledger(vec![entry(
        2,
        EVENT_1,
        &campaign_id,
        ConvergenceEvent::CampaignStarted(campaign_record(&campaign_id)),
    )]);
    assert!(noncontiguous.validate().is_err());

    let epoch = epoch_record();
    let duplicate_event_id = ledger(vec![
        entry(
            1,
            EVENT_1,
            &campaign_id,
            ConvergenceEvent::CampaignStarted(campaign_record(&campaign_id)),
        ),
        entry(
            2,
            EVENT_1,
            &campaign_id,
            ConvergenceEvent::EpochOpened(epoch),
        ),
    ]);
    assert!(duplicate_event_id.validate().is_err());
}

#[test]
fn convergence_ledger_rejects_event_before_start_and_bad_campaign_starts() {
    let campaign_a = campaign(CAMPAIGN_A);
    let campaign_b = campaign(CAMPAIGN_B);
    let event_before_start = ledger(vec![entry(
        1,
        EVENT_1,
        &campaign_a,
        ConvergenceEvent::EpochOpened(epoch_record()),
    )]);
    assert!(event_before_start.validate().is_err());

    let mismatched_start = ledger(vec![entry(
        1,
        EVENT_1,
        &campaign_a,
        ConvergenceEvent::CampaignStarted(campaign_record(&campaign_b)),
    )]);
    assert!(mismatched_start.validate().is_err());

    let duplicate_start = ledger(vec![
        entry(
            1,
            EVENT_1,
            &campaign_a,
            ConvergenceEvent::CampaignStarted(campaign_record(&campaign_a)),
        ),
        entry(
            2,
            EVENT_2,
            &campaign_a,
            ConvergenceEvent::CampaignStarted(campaign_record(&campaign_a)),
        ),
    ]);
    assert!(duplicate_start.validate().is_err());
}

#[test]
fn convergence_ledger_rejects_unknown_epoch_and_duplicate_cell() {
    let campaign_id = campaign(CAMPAIGN_A);
    let epoch = epoch_record();
    let cell = cell_record(&epoch);
    let unknown_epoch = ledger(vec![
        entry(
            1,
            EVENT_1,
            &campaign_id,
            ConvergenceEvent::CampaignStarted(campaign_record(&campaign_id)),
        ),
        entry(
            2,
            EVENT_2,
            &campaign_id,
            ConvergenceEvent::CoverageCellDefined(cell.clone()),
        ),
    ]);
    assert!(unknown_epoch.validate().is_err());

    let duplicate_cell = ledger(vec![
        entry(
            1,
            EVENT_1,
            &campaign_id,
            ConvergenceEvent::CampaignStarted(campaign_record(&campaign_id)),
        ),
        entry(
            2,
            EVENT_2,
            &campaign_id,
            ConvergenceEvent::EpochOpened(epoch),
        ),
        entry(
            3,
            EVENT_3,
            &campaign_id,
            ConvergenceEvent::CoverageCellDefined(cell.clone()),
        ),
        entry(
            4,
            EVENT_4,
            &campaign_id,
            ConvergenceEvent::CoverageCellDefined(cell),
        ),
    ]);
    assert!(duplicate_cell.validate().is_err());
}

#[test]
fn convergence_ledger_allows_same_epoch_and_cell_in_different_campaigns() {
    let campaign_a = campaign(CAMPAIGN_A);
    let campaign_b = campaign(CAMPAIGN_B);
    let epoch = epoch_record();
    let cell = cell_record(&epoch);
    let ledger = ledger(vec![
        entry(
            1,
            EVENT_1,
            &campaign_a,
            ConvergenceEvent::CampaignStarted(campaign_record(&campaign_a)),
        ),
        entry(
            2,
            EVENT_2,
            &campaign_a,
            ConvergenceEvent::EpochOpened(epoch.clone()),
        ),
        entry(
            3,
            EVENT_3,
            &campaign_a,
            ConvergenceEvent::CoverageCellDefined(cell.clone()),
        ),
        entry(
            4,
            EVENT_4,
            &campaign_b,
            ConvergenceEvent::CampaignStarted(campaign_record(&campaign_b)),
        ),
        entry(
            5,
            EVENT_5,
            &campaign_b,
            ConvergenceEvent::EpochOpened(epoch),
        ),
        entry(
            6,
            EVENT_6,
            &campaign_b,
            ConvergenceEvent::CoverageCellDefined(cell),
        ),
    ]);

    ledger
        .validate()
        .expect("identity uniqueness must be campaign-scoped");
}

#[test]
fn convergence_ledger_rejects_future_schema_unknown_fields_variants_and_missing_evidence() {
    let mut future = serde_json::to_value(ConvergenceLedger::empty()).unwrap();
    future["schema_version"] = json!(CONVERGENCE_LEDGER_SCHEMA_VERSION + 1);
    let future: ConvergenceLedger = serde_json::from_value(future).unwrap();
    assert!(future.validate().is_err());

    let mut unknown_ledger = serde_json::to_value(ConvergenceLedger::empty()).unwrap();
    unknown_ledger["future"] = json!(true);
    assert!(serde_json::from_value::<ConvergenceLedger>(unknown_ledger).is_err());

    let campaign_id = campaign(CAMPAIGN_A);
    let valid = ledger(vec![entry(
        1,
        EVENT_1,
        &campaign_id,
        ConvergenceEvent::CampaignStarted(campaign_record(&campaign_id)),
    )]);
    let mut unknown_entry = serde_json::to_value(&valid).unwrap();
    unknown_entry["entries"][0]["future"] = json!(true);
    assert!(serde_json::from_value::<ConvergenceLedger>(unknown_entry).is_err());

    let mut unknown_variant = serde_json::to_value(&valid).unwrap();
    unknown_variant["entries"][0]["event"]["kind"] = json!("campaign_paused");
    assert!(serde_json::from_value::<ConvergenceLedger>(unknown_variant).is_err());

    let mut missing_ledger_field = serde_json::to_value(&valid).unwrap();
    missing_ledger_field
        .as_object_mut()
        .unwrap()
        .remove("entries");
    assert!(serde_json::from_value::<ConvergenceLedger>(missing_ledger_field).is_err());

    let mut missing_entry_field = serde_json::to_value(valid).unwrap();
    missing_entry_field["entries"][0]
        .as_object_mut()
        .unwrap()
        .remove("recorded_at");
    assert!(serde_json::from_value::<ConvergenceLedger>(missing_entry_field).is_err());
}

#[test]
fn convergence_ledger_rejects_tampered_epoch_and_cell_records() {
    let campaign_id = campaign(CAMPAIGN_A);
    let epoch = epoch_record();
    let cell = cell_record(&epoch);

    let mut tampered_epoch_value = serde_json::to_value(&epoch).unwrap();
    tampered_epoch_value["id"] = Value::String(digest('f').to_string());
    let tampered_epoch: EpochRecord = serde_json::from_value(tampered_epoch_value).unwrap();
    let epoch_ledger = ledger(vec![
        entry(
            1,
            EVENT_1,
            &campaign_id,
            ConvergenceEvent::CampaignStarted(campaign_record(&campaign_id)),
        ),
        entry(
            2,
            EVENT_2,
            &campaign_id,
            ConvergenceEvent::EpochOpened(tampered_epoch),
        ),
    ]);
    assert!(epoch_ledger.validate().is_err());

    let mut tampered_cell_value = serde_json::to_value(&cell).unwrap();
    tampered_cell_value["id"] = Value::String(digest('f').to_string());
    let tampered_cell: CoverageCellRecord = serde_json::from_value(tampered_cell_value).unwrap();
    let cell_ledger = ledger(vec![
        entry(
            1,
            EVENT_1,
            &campaign_id,
            ConvergenceEvent::CampaignStarted(campaign_record(&campaign_id)),
        ),
        entry(
            2,
            EVENT_2,
            &campaign_id,
            ConvergenceEvent::EpochOpened(epoch),
        ),
        entry(
            3,
            EVENT_3,
            &campaign_id,
            ConvergenceEvent::CoverageCellDefined(tampered_cell),
        ),
    ]);
    assert!(cell_ledger.validate().is_err());
}
