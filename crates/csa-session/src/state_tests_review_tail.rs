// ── Genealogy fork fields ──────────────────────────────────────

#[test]
fn test_genealogy_backward_compat_without_fork_fields() {
    let toml_str = r#"
depth = 1
parent_session_id = "01PARENT"
"#;
    let genealogy: Genealogy =
        toml::from_str(toml_str).expect("should deserialize without fork fields");
    assert_eq!(genealogy.parent_session_id, Some("01PARENT".to_string()));
    assert_eq!(genealogy.depth, 1);
    assert_eq!(genealogy.fork_of_session_id, None);
    assert_eq!(genealogy.fork_provider_session_id, None);
    assert!(!genealogy.is_fork());
    assert_eq!(genealogy.fork_source(), None);
}

#[test]
fn test_genealogy_with_fork_fields_roundtrip() {
    let genealogy = Genealogy {
        parent_session_id: Some("01PARENT".to_string()),
        depth: 1,
        fork_of_session_id: Some("01SOURCE".to_string()),
        fork_provider_session_id: Some("provider-abc-123".to_string()),
    };

    let serialized = toml::to_string(&genealogy).expect("serialize");
    let deserialized: Genealogy = toml::from_str(&serialized).expect("deserialize");

    assert_eq!(deserialized.parent_session_id, Some("01PARENT".to_string()));
    assert_eq!(deserialized.depth, 1);
    assert_eq!(
        deserialized.fork_of_session_id,
        Some("01SOURCE".to_string())
    );
    assert_eq!(
        deserialized.fork_provider_session_id,
        Some("provider-abc-123".to_string())
    );
}

#[test]
fn test_genealogy_is_fork_true() {
    let genealogy = Genealogy {
        fork_of_session_id: Some("01SOURCE".to_string()),
        ..Default::default()
    };
    assert!(genealogy.is_fork());
    assert_eq!(genealogy.fork_source(), Some("01SOURCE"));
}

#[test]
fn test_genealogy_is_fork_false_for_spawn_child() {
    let genealogy = Genealogy {
        parent_session_id: Some("01PARENT".to_string()),
        depth: 1,
        ..Default::default()
    };
    assert!(!genealogy.is_fork());
    assert_eq!(genealogy.fork_source(), None);
}

#[test]
fn test_genealogy_skip_serializing_none_fork_fields() {
    let genealogy = Genealogy::default();
    let serialized = toml::to_string(&genealogy).expect("serialize");
    assert!(
        !serialized.contains("fork_of_session_id"),
        "None fork fields should be skipped in serialization"
    );
    assert!(!serialized.contains("fork_provider_session_id"));
}

// ── ReviewSessionMeta tests ──────────────────────────────────────

#[test]
fn review_session_meta_serde_roundtrip() {
    let meta = ReviewSessionMeta {
        session_id: "01JABCDEF0123456789ABCDEFG".to_string(),
        head_sha: "abc123def456".to_string(),
        decision: "fail".to_string(),
        verdict: "HAS_ISSUES".to_string(),
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
        tool: "claude-code".to_string(),
        scope: "range:main...HEAD".to_string(),
        exit_code: 1,
        fix_attempted: true,
        fix_rounds: 2,
        review_iterations: 3,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: Some("sha256:abc123".to_string()),
    };

    let json = serde_json::to_string_pretty(&meta).expect("serialize");
    let decoded: ReviewSessionMeta = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(decoded, meta);
}

#[test]
fn review_session_meta_write_and_read() {
    let td = tempfile::tempdir().expect("tempdir");
    let meta = ReviewSessionMeta {
        session_id: "01JABCDEF0123456789ABCDEFG".to_string(),
        head_sha: "deadbeef".to_string(),
        decision: "pass".to_string(),
        verdict: "CLEAN".to_string(),
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
        tool: "codex".to_string(),
        scope: "uncommitted".to_string(),
        exit_code: 0,
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations: 1,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
    };

    write_review_meta(td.path(), &meta).expect("write");

    let path = td.path().join("review_meta.json");
    assert!(path.exists(), "review_meta.json should be created");

    let content = std::fs::read_to_string(&path).expect("read");
    let decoded: ReviewSessionMeta = serde_json::from_str(&content).expect("parse");
    assert_eq!(decoded.session_id, meta.session_id);
    assert_eq!(decoded.decision, "pass");
    assert!(!decoded.fix_attempted);
}

#[test]
fn review_session_meta_overwrite_on_fix_round() {
    let td = tempfile::tempdir().expect("tempdir");

    let meta1 = ReviewSessionMeta {
        session_id: "SESSION1".to_string(),
        head_sha: "aaa".to_string(),
        decision: "fail".to_string(),
        verdict: "HAS_ISSUES".to_string(),
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
        tool: "claude-code".to_string(),
        scope: "base:main".to_string(),
        exit_code: 1,
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations: 1,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
    };
    write_review_meta(td.path(), &meta1).expect("write initial");

    let meta2 = ReviewSessionMeta {
        session_id: "SESSION1".to_string(),
        head_sha: "bbb".to_string(),
        decision: "pass".to_string(),
        verdict: "CLEAN".to_string(),
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
        tool: "claude-code".to_string(),
        scope: "base:main".to_string(),
        exit_code: 0,
        fix_attempted: true,
        fix_rounds: 1,
        review_iterations: 1,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: Some("sha256:def456".to_string()),
    };
    write_review_meta(td.path(), &meta2).expect("write after fix");

    let content = std::fs::read_to_string(td.path().join("review_meta.json")).expect("read");
    let decoded: ReviewSessionMeta = serde_json::from_str(&content).expect("parse");
    assert_eq!(decoded.decision, "pass");
    assert!(decoded.fix_attempted);
    assert_eq!(decoded.fix_rounds, 1);
    assert_eq!(decoded.review_iterations, 1);
    assert_eq!(decoded.head_sha, "bbb");
}

#[test]
fn review_session_meta_missing_review_iterations_defaults_to_one() {
    let json = r#"{
        "session_id": "SESSION1",
        "head_sha": "aaa",
        "decision": "fail",
        "verdict": "HAS_ISSUES",
        "tool": "codex",
        "scope": "base:main",
        "exit_code": 1,
        "fix_attempted": false,
        "fix_rounds": 0,
        "timestamp": "2026-04-12T00:00:00Z"
    }"#;

    let decoded: ReviewSessionMeta = serde_json::from_str(json).expect("parse");
    assert_eq!(decoded.review_iterations, 1);
}
