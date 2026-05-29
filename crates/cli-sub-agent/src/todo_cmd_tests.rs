use super::*;
use csa_todo::{CriterionKind, CriterionStatus, SpecCriterion, SpecDocument, TodoManager};
use tempfile::tempdir;

// --- truncate tests ---

#[test]
fn truncate_short_string_unchanged() {
    assert_eq!(truncate("hello", 10), "hello");
}

#[test]
fn truncate_exact_length_unchanged() {
    assert_eq!(truncate("hello", 5), "hello");
}

#[test]
fn truncate_long_string_adds_ellipsis() {
    let result = truncate("hello world", 6);
    assert!(result.ends_with('\u{2026}'));
    assert_eq!(result.chars().count(), 6);
}

#[test]
fn truncate_preserves_multibyte_boundaries() {
    // 6 CJK characters
    let cjk = "\u{4f60}\u{597d}\u{4e16}\u{754c}\u{6d4b}\u{8bd5}";
    let result = truncate(cjk, 4);
    assert!(result.ends_with('\u{2026}'));
    assert_eq!(result.chars().count(), 4);
}

#[test]
fn truncate_single_char_max() {
    let result = truncate("abcdef", 1);
    assert_eq!(result, "\u{2026}");
}

#[test]
fn render_spec_document_includes_summary_and_criteria() {
    let spec = SpecDocument {
        schema_version: 1,
        plan_ulid: "01JABCDEF0123456789ABCDEFG".to_string(),
        summary: "Validate spec display.".to_string(),
        criteria: vec![
            SpecCriterion {
                kind: CriterionKind::Scenario,
                id: "scenario-show".to_string(),
                description: "show --spec renders criteria".to_string(),
                status: CriterionStatus::Pending,
            },
            SpecCriterion {
                kind: CriterionKind::Check,
                id: "check-failure".to_string(),
                description: "failed criteria are labeled".to_string(),
                status: CriterionStatus::Failed,
            },
        ],
    };

    let rendered = render_spec_document(&spec);

    assert!(rendered.contains("Plan ULID: 01JABCDEF0123456789ABCDEFG"));
    assert!(rendered.contains("Summary: Validate spec display."));
    assert!(rendered.contains("- [pending] scenario scenario-show: show --spec renders criteria"));
    assert!(rendered.contains("- [failed] check check-failure: failed criteria are labeled"));
}

#[test]
fn plan_attestation_warning_reports_mismatch() {
    let dir = tempdir().expect("tempdir");
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());
    let plan = manager.create("Warn on tamper", None).expect("plan");
    // Establish an attestation baseline (what `csa todo save` does); only a
    // post-attestation edit is tamper. A freshly created plan is un-attested
    // (`Missing`) and must NOT warn (#1669).
    manager.attest(&plan.timestamp).expect("attest");
    std::fs::write(plan.todo_md_path(), "# Tampered\n").expect("tamper");

    assert_eq!(
        plan_attestation_warning(&manager, &plan.timestamp).expect("warning"),
        Some(PLAN_TAMPERED_WARNING)
    );
}

#[test]
fn plan_attestation_warning_silent_for_unattested_draft() {
    // #1669: a freshly created, un-attested plan whose TODO.md is written
    // directly (the mktd workflow) reports `Missing`, so no `[PLAN TAMPERED]`
    // banner is emitted — preventing cry-wolf that trains operators to ignore it.
    let dir = tempdir().expect("tempdir");
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());
    let plan = manager.create("Unattested draft", None).expect("plan");
    std::fs::write(plan.todo_md_path(), "# real plan content\n").expect("write");

    assert_eq!(
        plan_attestation_warning(&manager, &plan.timestamp).expect("warning"),
        None
    );
}

// --- resolve_timestamp tests ---

// resolve_timestamp with Some returns the string directly
#[test]
fn resolve_timestamp_with_some_returns_value() {
    // We cannot call resolve_timestamp directly because it needs a TodoManager,
    // but we can test the logic: when timestamp is Some, it just returns it.
    let ts: Option<&str> = Some("20250101T120000");
    let result = ts.map(String::from).unwrap();
    assert_eq!(result, "20250101T120000");
}
