use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_todo::{CriterionKind, CriterionStatus, SpecCriterion, SpecDocument, TodoManager};
use tempfile::tempdir;

#[test]
fn handle_persist_reads_artifacts_and_commits_generated_plan() {
    let project_dir = tempdir().expect("project tempdir");
    let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
    let manager = TodoManager::new(project_dir.path()).expect("todo manager");
    csa_todo::git::ensure_git_init(manager.todos_dir()).expect("init todos git");
    let plan = manager
        .create("Persist generated plan", Some("fix/persist-generated-plan"))
        .expect("create plan");
    csa_todo::git::save(manager.todos_dir(), &plan.timestamp, "create plan")
        .expect("save initial plan")
        .expect("initial plan should commit");

    let artifact_dir = project_dir.path().join("session-output");
    std::fs::create_dir_all(&artifact_dir).expect("create artifact dir");
    let todo_file = artifact_dir.join("TODO.md");
    let spec_file = artifact_dir.join("spec.toml");
    std::fs::write(
        &todo_file,
        "# Persisted plan\n\n## Tasks\n\n- [ ] Use csa todo persist.\n  DONE WHEN: csa todo show renders this saved task.\n",
    )
    .expect("write TODO artifact");
    let spec = SpecDocument {
        schema_version: 1,
        plan_ulid: plan.timestamp.clone(),
        summary: "Persist generated plan artifacts.".to_string(),
        criteria: vec![SpecCriterion {
            kind: CriterionKind::Check,
            id: "check-persist".to_string(),
            description: "Generated TODO/spec files are committed through csa todo persist."
                .to_string(),
            status: CriterionStatus::Pending,
        }],
    };
    std::fs::write(
        &spec_file,
        toml::to_string_pretty(&spec).expect("serialize spec"),
    )
    .expect("write spec artifact");

    handle_persist(
        plan.timestamp.clone(),
        todo_file.display().to_string(),
        spec_file.display().to_string(),
        None,
        Some("finalize generated plan".to_string()),
        Some(project_dir.path().display().to_string()),
    )
    .expect("persist generated plan");

    let saved_todo = std::fs::read_to_string(plan.todo_md_path()).expect("read persisted TODO.md");
    assert!(saved_todo.contains("Use csa todo persist."));
    assert_eq!(
        manager.load_spec(&plan.timestamp).expect("load spec"),
        Some(spec)
    );
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(manager.todos_dir())
        .output()
        .expect("git status");
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "todos git status should be clean after handle_persist"
    );
}
