use csa_todo::{
    CriterionKind, CriterionStatus, GeneratedPlanPersistRequest, SpecCriterion, SpecDocument,
    TodoManager,
};

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

#[test]
fn generated_plan_persist_survives_later_simulated_gate_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = TodoManager::with_base_dir(dir.path().to_path_buf());
    csa_todo::git::ensure_git_init(manager.todos_dir()).expect("init todos git");
    let plan = manager
        .create("Generated plan persistence", Some("fix/generated-plan"))
        .expect("create plan");
    csa_todo::git::save(manager.todos_dir(), &plan.timestamp, "create plan")
        .expect("save initial plan")
        .expect("initial plan should commit");

    let todo_content = "# Generated plan\n\n## Tasks\n\n- [ ] Persist generated plan.\n  DONE WHEN: csa todo show renders this task.\n";
    let spec = sample_spec(&plan.timestamp);
    let persisted = manager
        .persist_generated_plan(
            &plan.timestamp,
            GeneratedPlanPersistRequest {
                todo_content,
                spec: &spec,
                epic_plan: None,
            },
        )
        .expect("persist generated plan");
    let file_refs: Vec<&str> = persisted.changed_files.iter().map(String::as_str).collect();
    csa_todo::git::save_files(
        manager.todos_dir(),
        &plan.timestamp,
        &file_refs,
        "finalize generated plan",
    )
    .expect("save generated plan")
    .expect("generated plan should commit");

    let simulated_post_exec_gate = Err::<(), _>(anyhow::anyhow!("post-exec gate failed"));
    assert!(simulated_post_exec_gate.is_err());

    let saved_todo =
        std::fs::read_to_string(plan.todo_md_path()).expect("TODO.md should remain persisted");
    assert!(saved_todo.contains("Persist generated plan."));
    assert_eq!(
        manager
            .load_spec(&plan.timestamp)
            .expect("load persisted spec"),
        Some(spec)
    );

    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(manager.todos_dir())
        .output()
        .expect("git status");
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "todos git status should be clean after atomic save"
    );
}
