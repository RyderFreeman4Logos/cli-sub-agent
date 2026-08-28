use csa_todo::{CriterionKind, CriterionStatus, SpecCriterion, SpecDocument};
use tempfile::tempdir;

use super::*;

fn sample_spec_document(plan_ulid: &str, criterion_id: &str) -> SpecDocument {
    SpecDocument {
        schema_version: 1,
        plan_ulid: plan_ulid.to_string(),
        summary: format!("Spec summary for {plan_ulid}"),
        criteria: vec![SpecCriterion {
            kind: CriterionKind::Scenario,
            id: criterion_id.to_string(),
            description: format!("Criterion {criterion_id} must be satisfied."),
            status: CriterionStatus::Pending,
        }],
    }
}

#[test]
fn daemon_review_context_snapshot_survives_source_replacement() {
    let project = tempdir().unwrap();
    let session = tempdir().unwrap();
    let path = project.path().join("TODO.md");
    std::fs::write(&path, "admitted context").unwrap();
    let parent = resolve_review_context(Some("TODO.md"), project.path(), false)
        .unwrap()
        .unwrap();

    parent.persist_daemon_snapshot(session.path()).unwrap();
    std::fs::write(&path, "replacement context").unwrap();
    let child = ResolvedReviewContext::load_daemon_snapshot(session.path())
        .unwrap()
        .expect("daemon child should receive the admitted snapshot");

    assert_eq!(child.snapshot(), "admitted context");
    assert_eq!(child.digest, parent.digest);
    assert_eq!(child.kind, parent.kind);
}

#[test]
fn resolve_review_context_accepts_dot_relative_explicit_paths() {
    let project = tempdir().unwrap();
    std::fs::write(project.path().join("TODO.md"), "todo context").unwrap();
    std::fs::write(
        project.path().join("spec.toml"),
        toml::to_string_pretty(&sample_spec_document(
            "01JTESTPLAN0000000000000002",
            "criterion-dot-relative",
        ))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(project.path().join("prompt.md"), "prompt context").unwrap();

    for path in ["./TODO.md", "./spec.toml", "./prompt.md"] {
        let context = resolve_review_context(Some(path), project.path(), false)
            .unwrap_or_else(|error| panic!("{path} should be admitted: {error:#}"))
            .expect("explicit path should resolve");
        assert!(!context.snapshot().is_empty(), "{path}");
    }

    let parent = resolve_review_context(Some("../TODO.md"), project.path(), false)
        .expect_err("parent traversal must stay rejected");
    assert!(format!("{parent:#}").contains("must be a file beneath"));
}
