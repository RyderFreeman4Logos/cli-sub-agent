use csa_todo::{CriterionKind, CriterionStatus, SpecCriterion, SpecDocument, TodoManager};

fn sample_spec(plan_ulid: &str) -> SpecDocument {
    SpecDocument {
        schema_version: 1,
        plan_ulid: plan_ulid.to_string(),
        summary: "Integration-test spec lifecycle coverage.".to_string(),
        criteria: vec![
            SpecCriterion {
                kind: CriterionKind::Scenario,
                id: "scenario-review-red-team".to_string(),
                description: "Red-team review mode surfaces adversarial findings.".to_string(),
                status: CriterionStatus::Pending,
            },
            SpecCriterion {
                kind: CriterionKind::Property,
                id: "property-roundtrip".to_string(),
                description: "Persisted spec.toml roundtrips without field loss.".to_string(),
                status: CriterionStatus::Verified,
            },
            SpecCriterion {
                kind: CriterionKind::Check,
                id: "check-spec-path".to_string(),
                description: "spec.toml lives alongside TODO.md in the plan directory.".to_string(),
                status: CriterionStatus::Pending,
            },
        ],
    }
}

#[test]
fn spec_lifecycle_roundtrips_through_todo_manager_storage() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());
    let plan = manager
        .create(
            "Spec lifecycle integration",
            Some("feat/spec-intent-review"),
        )
        .unwrap();

    assert_eq!(
        manager.load_spec(&plan.timestamp).unwrap(),
        None,
        "missing spec.toml should deserialize as None"
    );

    let spec_path = manager.spec_path(&plan.timestamp);
    assert_eq!(spec_path, plan.todo_dir.join("spec.toml"));
    assert_eq!(spec_path.parent(), plan.todo_md_path().parent());

    let spec = sample_spec(&plan.timestamp);
    manager.save_spec(&plan.timestamp, &spec).unwrap();

    let persisted = std::fs::read_to_string(&spec_path).expect("spec.toml should be written");
    assert!(persisted.contains("scenario-review-red-team"));
    assert!(persisted.contains("property-roundtrip"));
    assert!(persisted.contains("check-spec-path"));

    let loaded = manager.load_spec(&plan.timestamp).unwrap();
    assert_eq!(loaded, Some(spec));
}
