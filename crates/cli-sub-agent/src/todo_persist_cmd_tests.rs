use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_todo::{CriterionKind, CriterionStatus, SpecCriterion, SpecDocument, TodoManager};
use tempfile::tempdir;

fn valid_han_summary() -> String {
    "\u{9a8c}\u{8bc1}\u{751f}\u{6210}\u{8ba1}\u{5212}".to_string()
}

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
        summary: valid_han_summary(),
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
        false,
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

#[test]
fn handle_persist_dry_run_validates_without_committing() -> anyhow::Result<()> {
    let project_dir = tempdir()?;
    let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
    let manager = TodoManager::new(project_dir.path())?;
    csa_todo::git::ensure_git_init(manager.todos_dir())?;
    let plan = manager.create("Dry-run generated plan", Some("fix/persist-dry-run"))?;
    csa_todo::git::save(manager.todos_dir(), &plan.timestamp, "create plan")?
        .ok_or_else(|| anyhow::anyhow!("initial plan should commit"))?;
    let head_before = git_head(manager.todos_dir())?;
    let original_todo = std::fs::read_to_string(plan.todo_md_path())?;

    let artifact_dir = project_dir.path().join("session-output");
    std::fs::create_dir_all(&artifact_dir)?;
    let todo_file = artifact_dir.join("TODO.md");
    let spec_file = artifact_dir.join("spec.toml");
    std::fs::write(
        &todo_file,
        "# Dry-run plan\n\n## Tasks\n\n- [ ] Validate without committing.\n  DONE WHEN: csa todo persist --dry-run exits successfully without writing TODO state.\n",
    )?;
    let spec = SpecDocument {
        schema_version: 1,
        plan_ulid: plan.timestamp.clone(),
        summary: valid_han_summary(),
        criteria: vec![SpecCriterion {
            kind: CriterionKind::Check,
            id: "check-dry-run".to_string(),
            description: "Dry-run validates artifacts without committing.".to_string(),
            status: CriterionStatus::Pending,
        }],
    };
    std::fs::write(&spec_file, toml::to_string_pretty(&spec)?)?;

    handle_persist(
        plan.timestamp.clone(),
        todo_file.display().to_string(),
        spec_file.display().to_string(),
        None,
        Some("validate generated plan".to_string()),
        true,
        Some(project_dir.path().display().to_string()),
    )?;

    assert_eq!(
        head_before,
        git_head(manager.todos_dir())?,
        "dry-run must not create a todos-git commit"
    );
    assert_eq!(
        original_todo,
        std::fs::read_to_string(plan.todo_md_path())?,
        "dry-run must not rewrite TODO.md"
    );
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(manager.todos_dir())
        .output()?;
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "todos git status should stay clean after dry-run"
    );
    Ok(())
}

#[test]
fn handle_persist_dry_run_recovers_empty_spec_artifact_from_raw_spec() -> anyhow::Result<()> {
    let project_dir = tempdir()?;
    let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
    let manager = TodoManager::new(project_dir.path())?;
    csa_todo::git::ensure_git_init(manager.todos_dir())?;
    let plan = manager.create("Dry-run raw spec recovery", Some("fix/raw-spec-recovery"))?;
    csa_todo::git::save(manager.todos_dir(), &plan.timestamp, "create plan")?
        .ok_or_else(|| anyhow::anyhow!("initial plan should commit"))?;
    let head_before = git_head(manager.todos_dir())?;
    let original_todo = std::fs::read_to_string(plan.todo_md_path())?;

    let artifact_dir = project_dir.path().join("session-output").join("mktd-save");
    std::fs::create_dir_all(&artifact_dir)?;
    let todo_file = artifact_dir.join("TODO.md");
    let spec_file = artifact_dir.join("spec.toml");
    let raw_spec_file = artifact_dir.join("spec.raw.txt");
    std::fs::write(
        &todo_file,
        "# Raw spec recovery\n\n## Tasks\n\n- [ ] Recover the raw spec artifact.\n  DONE WHEN: csa todo persist --dry-run validates the unambiguous raw spec without writing TODO state.\n",
    )?;
    std::fs::write(&spec_file, "")?;
    std::fs::write(
        &raw_spec_file,
        format!(
            "I will extract the TODO criteria.\n<!-- CSA:SECTION:details -->\n{}\n<!-- CSA:SECTION:details:END -->\n",
            toml::to_string_pretty(&SpecDocument {
                schema_version: 1,
                plan_ulid: plan.timestamp.clone(),
                summary: valid_han_summary(),
                criteria: vec![SpecCriterion {
                    kind: CriterionKind::Check,
                    id: "check-raw-spec-recovery".to_string(),
                    description:
                        "Dry-run recovers an unambiguous raw spec artifact beside spec.toml."
                            .to_string(),
                    status: CriterionStatus::Pending,
                }],
            })?
        ),
    )?;

    handle_persist(
        plan.timestamp.clone(),
        todo_file.display().to_string(),
        spec_file.display().to_string(),
        None,
        Some("validate raw spec recovery".to_string()),
        true,
        Some(project_dir.path().display().to_string()),
    )?;

    assert_eq!(
        head_before,
        git_head(manager.todos_dir())?,
        "dry-run raw spec recovery must not create a todos-git commit"
    );
    assert_eq!(
        original_todo,
        std::fs::read_to_string(plan.todo_md_path())?,
        "dry-run raw spec recovery must not rewrite TODO.md"
    );
    Ok(())
}

#[test]
fn handle_persist_dry_run_rejects_invalid_spec_summary_without_committing() -> anyhow::Result<()> {
    let project_dir = tempdir()?;
    let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
    let manager = TodoManager::new(project_dir.path())?;
    csa_todo::git::ensure_git_init(manager.todos_dir())?;
    let plan = manager.create("Dry-run summary gate", Some("fix/persist-summary-gate"))?;
    csa_todo::git::save(manager.todos_dir(), &plan.timestamp, "create plan")?
        .ok_or_else(|| anyhow::anyhow!("initial plan should commit"))?;
    let head_before = git_head(manager.todos_dir())?;
    let original_todo = std::fs::read_to_string(plan.todo_md_path())?;

    let artifact_dir = project_dir.path().join("session-output");
    std::fs::create_dir_all(&artifact_dir)?;
    let todo_file = artifact_dir.join("TODO.md");
    let spec_file = artifact_dir.join("spec.toml");
    std::fs::write(
        &todo_file,
        "# Summary gate\n\n## Tasks\n\n- [ ] Reject invalid spec summaries.\n  DONE WHEN: csa todo persist --dry-run rejects empty and no-Han summaries.\n",
    )?;

    for (summary, expected) in [
        ("", "generated spec summary is empty"),
        (
            "English only summary",
            "generated spec summary lacks Han characters",
        ),
    ] {
        let spec = SpecDocument {
            schema_version: 1,
            plan_ulid: plan.timestamp.clone(),
            summary: summary.to_string(),
            criteria: vec![SpecCriterion {
                kind: CriterionKind::Check,
                id: "check-summary-gate".to_string(),
                description: "Dry-run rejects spec summaries that cannot become CSA-Criteria."
                    .to_string(),
                status: CriterionStatus::Pending,
            }],
        };
        std::fs::write(&spec_file, toml::to_string_pretty(&spec)?)?;

        let err = handle_persist(
            plan.timestamp.clone(),
            todo_file.display().to_string(),
            spec_file.display().to_string(),
            None,
            Some("validate generated plan".to_string()),
            true,
            Some(project_dir.path().display().to_string()),
        )
        .expect_err("dry-run must reject invalid spec summaries");
        assert!(
            err.to_string().contains(expected),
            "diagnostic should contain {expected}: {err}"
        );
    }

    assert_eq!(
        head_before,
        git_head(manager.todos_dir())?,
        "dry-run summary rejection must not create a todos-git commit"
    );
    assert_eq!(
        original_todo,
        std::fs::read_to_string(plan.todo_md_path())?,
        "dry-run summary rejection must not rewrite TODO.md"
    );
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(manager.todos_dir())
        .output()?;
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "todos git status should stay clean after dry-run summary rejection"
    );

    Ok(())
}

#[test]
fn handle_persist_dry_run_rejects_unsupported_schema_without_committing() -> anyhow::Result<()> {
    let project_dir = tempdir()?;
    let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
    let manager = TodoManager::new(project_dir.path())?;
    csa_todo::git::ensure_git_init(manager.todos_dir())?;
    let plan = manager.create("Dry-run schema gate", Some("fix/persist-schema-gate"))?;
    csa_todo::git::save(manager.todos_dir(), &plan.timestamp, "create plan")?
        .ok_or_else(|| anyhow::anyhow!("initial plan should commit"))?;
    let head_before = git_head(manager.todos_dir())?;
    let original_todo = std::fs::read_to_string(plan.todo_md_path())?;

    let artifact_dir = project_dir.path().join("session-output");
    std::fs::create_dir_all(&artifact_dir)?;
    let todo_file = artifact_dir.join("TODO.md");
    let spec_file = artifact_dir.join("spec.toml");
    std::fs::write(
        &todo_file,
        "# Schema gate\n\n## Tasks\n\n- [ ] Reject unsupported spec schemas.\n  DONE WHEN: csa todo persist --dry-run rejects schema_version other than 1.\n",
    )?;
    let spec = SpecDocument {
        schema_version: 2,
        plan_ulid: plan.timestamp.clone(),
        summary: valid_han_summary(),
        criteria: vec![SpecCriterion {
            kind: CriterionKind::Check,
            id: "check-schema-gate".to_string(),
            description: "Dry-run rejects unsupported spec schema versions.".to_string(),
            status: CriterionStatus::Pending,
        }],
    };
    std::fs::write(&spec_file, toml::to_string_pretty(&spec)?)?;

    let err = handle_persist(
        plan.timestamp.clone(),
        todo_file.display().to_string(),
        spec_file.display().to_string(),
        None,
        Some("validate generated plan".to_string()),
        true,
        Some(project_dir.path().display().to_string()),
    )
    .expect_err("dry-run must reject unsupported spec schema versions");
    assert!(
        err.to_string()
            .contains("unsupported spec schema_version 2; expected 1"),
        "diagnostic should identify the unsupported schema version: {err}"
    );

    assert_eq!(
        head_before,
        git_head(manager.todos_dir())?,
        "dry-run schema rejection must not create a todos-git commit"
    );
    assert_eq!(
        original_todo,
        std::fs::read_to_string(plan.todo_md_path())?,
        "dry-run schema rejection must not rewrite TODO.md"
    );
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(manager.todos_dir())
        .output()?;
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "todos git status should stay clean after dry-run schema rejection"
    );

    Ok(())
}

#[test]
fn handle_persist_fires_todo_save_hook_after_commit_once() -> anyhow::Result<()> {
    let project_dir = tempdir()?;
    let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
    let manager = TodoManager::new(project_dir.path())?;
    csa_todo::git::ensure_git_init(manager.todos_dir())?;
    let plan = manager.create("Persist hook plan", Some("fix/persist-hook-plan"))?;
    csa_todo::git::save(manager.todos_dir(), &plan.timestamp, "create plan")?
        .ok_or_else(|| anyhow::anyhow!("initial plan should commit"))?;

    let session_root = csa_session::get_session_root(project_dir.path())?;
    std::fs::create_dir_all(&session_root)?;
    let hook_log = project_dir.path().join("todo-save-hook.log");
    std::fs::write(
        session_root.join("hooks.toml"),
        format!(
            r#"[todo_save]
enabled = true
command = "test -z \"$(git -C {{todo_root}} status --porcelain)\" && printf '%s|%s|%s\\n' {{plan_id}} {{version}} {{message}} >> {}"
timeout_secs = 5
"#,
            hook_log.display()
        ),
    )?;

    let artifact_dir = project_dir.path().join("session-output");
    std::fs::create_dir_all(&artifact_dir)?;
    let todo_file = artifact_dir.join("TODO.md");
    let spec_file = artifact_dir.join("spec.toml");
    std::fs::write(
        &todo_file,
        "# Persisted hook plan\n\n## Tasks\n\n- [ ] Fire TodoSave from persist.\n  DONE WHEN: the todo_save hook runs after commit.\n",
    )?;
    let spec = SpecDocument {
        schema_version: 1,
        plan_ulid: plan.timestamp.clone(),
        summary: valid_han_summary(),
        criteria: vec![SpecCriterion {
            kind: CriterionKind::Check,
            id: "check-persist-hook".to_string(),
            description: "csa todo persist fires todo_save once after a successful commit."
                .to_string(),
            status: CriterionStatus::Pending,
        }],
    };
    std::fs::write(&spec_file, toml::to_string_pretty(&spec)?)?;

    handle_persist(
        plan.timestamp.clone(),
        todo_file.display().to_string(),
        spec_file.display().to_string(),
        None,
        Some("finalize generated plan".to_string()),
        false,
        Some(project_dir.path().display().to_string()),
    )?;

    let hook_output = std::fs::read_to_string(&hook_log)?;
    let lines: Vec<&str> = hook_output.lines().collect();
    assert_eq!(lines.len(), 1, "todo_save hook should run exactly once");
    assert_eq!(
        lines[0],
        format!("{}|2|finalize generated plan", plan.timestamp)
    );

    Ok(())
}

fn git_head(dir: &std::path::Path) -> anyhow::Result<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()?;
    anyhow::ensure!(out.status.success(), "git rev-parse HEAD failed");
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[test]
fn handle_persist_rejects_spec_section_marker_without_committing() -> anyhow::Result<()> {
    let project_dir = tempdir()?;
    let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
    let manager = TodoManager::new(project_dir.path())?;
    csa_todo::git::ensure_git_init(manager.todos_dir())?;
    let plan = manager.create("Reject marker spec", Some("fix/reject-marker-spec"))?;
    csa_todo::git::save(manager.todos_dir(), &plan.timestamp, "create plan")?
        .ok_or_else(|| anyhow::anyhow!("initial plan should commit"))?;
    let head_before = git_head(manager.todos_dir())?;

    let artifact_dir = project_dir.path().join("session-output");
    std::fs::create_dir_all(&artifact_dir)?;
    let todo_file = artifact_dir.join("TODO.md");
    let spec_file = artifact_dir.join("spec.toml");
    std::fs::write(
        &todo_file,
        "# Marker spec plan\n\n## Tasks\n\n- [ ] Reject non-TOML spec artifacts.\n  DONE WHEN: csa todo persist reports an artifact-shape diagnostic before commit.\n",
    )?;
    std::fs::write(
        &spec_file,
        "<!-- CSA:SECTION:summary -->\nMCP schema mismatch details are not TOML.\napi_key=fixture12345\nAuthorization: Bearer fixturebearertoken\n<!-- CSA:SECTION:summary:END -->\n",
    )?;

    let err = handle_persist(
        plan.timestamp.clone(),
        todo_file.display().to_string(),
        spec_file.display().to_string(),
        None,
        Some("finalize marker spec".to_string()),
        false,
        Some(project_dir.path().display().to_string()),
    )
    .expect_err("persist must reject CSA section marker spec artifacts");
    let message = err.to_string();
    assert!(message.contains("spec artifact-shape error"));
    assert!(message.contains("CSA section marker"));
    assert!(message.contains("inspect the producing step"));
    assert!(!message.contains("MCP schema mismatch details"));
    assert!(!message.contains("fixture12345"));
    assert!(!message.contains("fixturebearertoken"));
    assert!(!message.contains("Authorization: Bearer"));

    assert_eq!(
        head_before,
        git_head(manager.todos_dir())?,
        "no new todos-git commit may exist for a rejected spec artifact"
    );
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(manager.todos_dir())
        .output()?;
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "todos git status must stay clean when spec artifact shape is rejected"
    );
    Ok(())
}

/// Round-5 regression: a generated TODO that lacks a `DONE WHEN` clause must be
/// rejected fail-closed by `csa todo persist` BEFORE it commits, so no invalid
/// plan can enter the todos git history (the hard-gate contract). Proven by
/// asserting the todos repo HEAD is unchanged and the work tree stays clean.
#[test]
fn handle_persist_rejects_invalid_plan_without_committing() -> anyhow::Result<()> {
    let project_dir = tempdir()?;
    let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
    let manager = TodoManager::new(project_dir.path())?;
    csa_todo::git::ensure_git_init(manager.todos_dir())?;
    let plan = manager.create("Reject invalid plan", Some("fix/reject-invalid-plan"))?;
    // Commit the freshly created template plan so HEAD is well-defined.
    csa_todo::git::save(manager.todos_dir(), &plan.timestamp, "create plan")?
        .ok_or_else(|| anyhow::anyhow!("initial plan should commit"))?;
    let head_before = git_head(manager.todos_dir())?;

    let artifact_dir = project_dir.path().join("session-output");
    std::fs::create_dir_all(&artifact_dir)?;
    let todo_file = artifact_dir.join("TODO.md");
    let spec_file = artifact_dir.join("spec.toml");
    // Invalid: a real checkbox task but NO `DONE WHEN` completion clause.
    std::fs::write(
        &todo_file,
        "# Invalid plan\n\n## Tasks\n\n- [ ] Task without completion criteria.\n",
    )?;
    let spec = SpecDocument {
        schema_version: 1,
        plan_ulid: plan.timestamp.clone(),
        summary: valid_han_summary(),
        criteria: vec![SpecCriterion {
            kind: CriterionKind::Check,
            id: "check-invalid".to_string(),
            description: "Persist must reject this before committing.".to_string(),
            status: CriterionStatus::Pending,
        }],
    };
    std::fs::write(&spec_file, toml::to_string_pretty(&spec)?)?;

    let result = handle_persist(
        plan.timestamp.clone(),
        todo_file.display().to_string(),
        spec_file.display().to_string(),
        None,
        Some("finalize invalid plan".to_string()),
        false,
        Some(project_dir.path().display().to_string()),
    );
    assert!(
        result.is_err(),
        "persist must reject a generated TODO with no DONE WHEN clause"
    );

    // The rejected plan must NOT have produced a new commit in the todos repo.
    assert_eq!(
        head_before,
        git_head(manager.todos_dir())?,
        "no new todos-git commit may exist for the rejected invalid plan"
    );
    // And no half-written, uncommitted plan files may be left behind.
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(manager.todos_dir())
        .output()?;
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "todos git status must stay clean when persist rejects invalid content"
    );
    Ok(())
}
