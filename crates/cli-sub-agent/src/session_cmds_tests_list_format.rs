use super::super::list::{
    decode_ulid_created_at, format_compact_duration, format_elapsed, session_created_at,
};
use super::{sample_session_state, session_to_json};
use chrono::{Duration, Utc};
use csa_session::SessionPhase;

#[test]
fn format_compact_duration_basics() {
    assert_eq!(format_compact_duration(Duration::seconds(0)), "0s");
    assert_eq!(format_compact_duration(Duration::seconds(45)), "45s");
    assert_eq!(format_compact_duration(Duration::seconds(60)), "1m");
    assert_eq!(
        format_compact_duration(Duration::seconds(3 * 60 + 20)),
        "3m"
    );
    assert_eq!(format_compact_duration(Duration::seconds(3_600)), "1h");
    assert_eq!(
        format_compact_duration(Duration::seconds(3_600 + 23 * 60)),
        "1h23m"
    );
    assert_eq!(format_compact_duration(Duration::seconds(86_400)), "1d");
    assert_eq!(
        format_compact_duration(Duration::seconds(2 * 86_400 + 4 * 3_600)),
        "2d4h"
    );
}

#[test]
fn active_session_elapsed_uses_now_minus_created() {
    let mut session = sample_session_state();
    let now = Utc::now();
    session.phase = SessionPhase::Active;
    session.created_at = now - Duration::minutes(5);
    session.last_accessed = now - Duration::minutes(1);

    let elapsed = format_elapsed(&session, "Active", now);

    assert_eq!(elapsed, "5m");
}

#[test]
fn retired_session_elapsed_uses_last_accessed_minus_created() {
    let mut session = sample_session_state();
    let now = Utc::now();
    session.phase = SessionPhase::Retired;
    session.created_at = now - Duration::hours(2);
    session.last_accessed = now;

    let elapsed = format_elapsed(&session, "Retired", now + Duration::minutes(30));

    assert_eq!(elapsed, "2h");
}

#[test]
fn created_at_fallback_decodes_from_ulid_when_metadata_missing() {
    let ulid = ulid::Ulid::new();
    let decoded = decode_ulid_created_at(&ulid.to_string()).expect("decode");
    let now = Utc::now();

    assert!((now.timestamp_millis() - decoded.timestamp_millis()).abs() < 5_000);
}

#[test]
fn session_to_json_includes_started_at_and_elapsed() {
    let session = sample_session_state();
    let value = session_to_json(&session);

    assert_eq!(
        value.get("started_at"),
        Some(&serde_json::json!(session_created_at(&session)))
    );
    assert_eq!(value.get("elapsed").and_then(|v| v.as_str()), Some("0s"));
    assert_eq!(
        value.get("branch").and_then(|v| v.as_str()),
        Some("feature/x")
    );
    assert_eq!(
        value.get("task_type").and_then(|v| v.as_str()),
        Some("plan")
    );
}
