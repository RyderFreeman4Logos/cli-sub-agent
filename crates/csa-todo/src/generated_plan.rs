use anyhow::{Context, Result};
use chrono::Utc;

use crate::{
    ATTESTATION_FILE, EPIC_PLAN_FILE, EpicPlan, METADATA_FILE, SPEC_FILE, SpecDocument,
    TODO_MD_FILE, TodoManager, TodoPlan, atomic_write, validate_timestamp,
};

/// Generated plan content to persist for a TODO plan.
pub struct GeneratedPlanPersistRequest<'a> {
    pub todo_content: &'a str,
    pub spec: &'a SpecDocument,
    pub epic_plan: Option<&'a EpicPlan>,
}

/// Result of persisting generated plan content.
pub struct GeneratedPlanPersistResult {
    pub plan: TodoPlan,
    pub changed_files: Vec<String>,
}

impl TodoManager {
    /// Persist generated TODO/spec artifacts for an existing plan.
    ///
    /// Validates all structured inputs before mutating files, then writes each
    /// file through temp-file + rename while holding the TODO write lock.
    ///
    /// Convenience wrapper over [`persist_generated_plan_with`](Self::persist_generated_plan_with)
    /// for callers that only need the file writes and have no publish step to
    /// run under the same lock.
    pub fn persist_generated_plan(
        &self,
        timestamp: &str,
        request: GeneratedPlanPersistRequest<'_>,
    ) -> Result<GeneratedPlanPersistResult> {
        let (result, ()) = self.persist_generated_plan_with(timestamp, request, |_| Ok(()))?;
        Ok(result)
    }

    /// Persist generated TODO/spec artifacts AND run a caller `publish` step
    /// atomically, all under a single hold of the TODO write lock.
    ///
    /// The file-write phase is identical to
    /// [`persist_generated_plan`](Self::persist_generated_plan), but `publish`
    /// is invoked *before the write lock is released*, right after the atomic
    /// file writes succeed. This makes the file write and the caller's publish
    /// step (e.g. git stage + commit + the hook-trigger decision) one critical
    /// section: a concurrent TODO writer cannot interleave between the write and
    /// the commit and cause this call to publish the wrong snapshot (TOCTOU
    /// lost-update / corrupted audit history).
    ///
    /// `publish` receives the persist result (loaded plan + the list of changed
    /// files) and returns any caller-defined value (e.g. the commit hash). The
    /// write lock is released only after `publish` returns, on both the success
    /// and error paths (the lock guard in the write-lock helper drops on scope
    /// exit regardless of outcome).
    ///
    /// `publish` MUST NOT itself attempt to acquire the TODO write lock (e.g. by
    /// spawning `csa todo save`/`persist`), or it will deadlock against the held
    /// lock. Run any such side effect (e.g. firing the `TodoSave` hook) from the
    /// returned value, after this method returns.
    pub fn persist_generated_plan_with<T>(
        &self,
        timestamp: &str,
        request: GeneratedPlanPersistRequest<'_>,
        publish: impl FnOnce(&GeneratedPlanPersistResult) -> Result<T>,
    ) -> Result<(GeneratedPlanPersistResult, T)> {
        self.with_write_lock(|| {
            validate_timestamp(timestamp)?;
            if request.spec.plan_ulid != timestamp {
                anyhow::bail!(
                    "spec plan_ulid '{}' does not match TODO plan '{}'",
                    request.spec.plan_ulid,
                    timestamp
                );
            }
            if let Some(epic_plan) = request.epic_plan {
                epic_plan.validate()?;
            }
            // Fail-closed gate (#1820/#1822 hard-gate contract): reject generated
            // content that violates the plan's hard invariants BEFORE any file
            // write or commit. The mktd workflow validates the rendered artifacts
            // before calling persist, but this guards EVERY caller (e.g. a direct
            // `csa todo persist`) so an invalid plan can never enter the todos git
            // history via the `publish` commit closure below.
            validate_generated_plan_content(request.todo_content, request.spec)?;

            let mut plan = self.load_inner(timestamp)?;
            let spec_content =
                toml::to_string_pretty(request.spec).context("Failed to serialize spec")?;
            let epic_content = request
                .epic_plan
                .map(toml::to_string_pretty)
                .transpose()
                .context("Failed to serialize epic plan")?;

            atomic_write(&plan.todo_md_path(), request.todo_content.as_bytes())?;
            self.write_attestation_for_content(&plan, request.todo_content.as_bytes())?;
            plan.metadata.updated_at = Utc::now();
            self.write_metadata(&plan)?;
            atomic_write(&self.spec_path(timestamp), spec_content.as_bytes())?;

            let mut changed_files = vec![
                format!("{timestamp}/{TODO_MD_FILE}"),
                format!("{timestamp}/{ATTESTATION_FILE}"),
                format!("{timestamp}/{METADATA_FILE}"),
                format!("{timestamp}/{SPEC_FILE}"),
            ];
            if let Some(content) = epic_content {
                atomic_write(&self.epic_plan_path(timestamp), content.as_bytes())?;
                changed_files.push(format!("{timestamp}/{EPIC_PLAN_FILE}"));
            }

            let result = GeneratedPlanPersistResult {
                plan,
                changed_files,
            };
            // Run the caller's publish step (commit + hook-trigger decision)
            // BEFORE releasing the write lock, so the commit captures exactly
            // the content just written above.
            let published = publish(&result)?;
            Ok((result, published))
        })
    }
}

/// Reject generated plan content that violates the hard invariants every
/// persisted TODO plan must satisfy, evaluated BEFORE any file write or commit:
///
/// - at least one non-empty open checkbox task (`- [ ] <text>`),
/// - **every** open checkbox task carries its OWN mechanically-verifiable
///   `DONE WHEN:` completion clause (AGENTS.md Meta 005), and
/// - at least one spec criterion (so `csa todo show --spec` renders a non-empty
///   criteria list — the struct-level equivalent of the workflow's former
///   post-commit render check).
///
/// The `DONE WHEN:` check is **per-task**, not a single global mention: a plan
/// is rejected when ANY open task block lacks a clause, even if another task
/// carries one. The clause requires the colon followed by non-empty criteria
/// text, so a bare keyword mention in a subject (e.g.
/// `- [ ] Document DONE WHEN policy.`) does NOT satisfy the gate, and a clause
/// that lives on a *completed* (`- [x]`) task line cannot satisfy a sibling open
/// task. Completed tasks are exempt; only open tasks require a clause.
///
/// Returns an error so the caller's commit step never runs on invalid content.
fn validate_generated_plan_content(todo_content: &str, spec: &SpecDocument) -> Result<()> {
    let lines: Vec<&str> = todo_content.lines().collect();
    let mut open_task_count = 0usize;
    let mut missing_clause: Vec<&str> = Vec::new();

    let mut i = 0;
    while i < lines.len() {
        let Some(subject) = open_task_subject(lines[i]) else {
            i += 1;
            continue;
        };
        open_task_count += 1;
        // The task owns its checkbox line plus every following line up to — but
        // not including — the next checkbox (open OR completed) or section
        // heading. This isolates each open task's clause from its neighbours so
        // one task's `DONE WHEN:` can never cover another's gap.
        let start = i;
        i += 1;
        while i < lines.len() && !is_task_block_boundary(lines[i]) {
            i += 1;
        }
        let has_clause = lines[start..i]
            .iter()
            .any(|line| done_when_criteria(line).is_some());
        if !has_clause {
            missing_clause.push(subject);
        }
    }

    if open_task_count == 0 {
        anyhow::bail!("generated TODO has no non-empty checkbox task (`- [ ] <task>`)");
    }
    if !missing_clause.is_empty() {
        anyhow::bail!(
            "generated TODO has open task(s) without a mechanically-verifiable `DONE WHEN:` \
             completion clause: {}",
            missing_clause.join("; ")
        );
    }
    if spec.criteria.is_empty() {
        anyhow::bail!(
            "generated spec has no criteria; `csa todo show --spec` would render an empty plan"
        );
    }
    Ok(())
}

/// The subject text of an OPEN checkbox line (`- [ ] <subject>`), or `None` when
/// the line is not a non-empty open checkbox. Matches the column-0, unordered
/// `- [ ]` task format the mktd plan template renders and the workflow gate
/// checks (`^- \[ \] .+`).
fn open_task_subject(line: &str) -> Option<&str> {
    line.strip_prefix("- [ ] ").filter(|rest| !rest.is_empty())
}

/// True when `line` opens a new task-block boundary: any checkbox marker (open
/// or completed) or a Markdown section heading. Used to delimit one open task's
/// owned lines from the next.
fn is_task_block_boundary(line: &str) -> bool {
    line.starts_with("- [ ]")
        || line.starts_with("- [x]")
        || line.starts_with("- [X]")
        || line.starts_with('#')
}

/// The non-empty criteria text following a `DONE WHEN:` clause on `line`, or
/// `None` when the line has no clause or only empty/whitespace text after the
/// colon. The required colon means a bare keyword mention (e.g.
/// `... DONE WHEN policy.`) is deliberately NOT treated as a clause.
fn done_when_criteria(line: &str) -> Option<&str> {
    line.split_once("DONE WHEN:")
        .map(|(_, rest)| rest.trim())
        .filter(|rest| !rest.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CriterionKind, CriterionStatus, LOCK_FILE, SpecCriterion};
    use std::sync::atomic::{AtomicBool, Ordering};

    fn sample_spec(plan_ulid: &str) -> SpecDocument {
        SpecDocument {
            schema_version: 1,
            plan_ulid: plan_ulid.to_string(),
            summary: "lock-scope probe".to_string(),
            criteria: vec![SpecCriterion {
                kind: CriterionKind::Check,
                id: "check-lock-scope".to_string(),
                description: "Publish runs under the held write lock.".to_string(),
                status: CriterionStatus::Pending,
            }],
        }
    }

    #[test]
    fn validate_generated_plan_content_rejects_missing_invariants() {
        let spec = sample_spec("01JABCDEF0123456789ABCDEFG");
        // No checkbox task → rejected.
        assert!(validate_generated_plan_content("# Plan\n\nDONE WHEN: x\n", &spec).is_err());
        // No `DONE WHEN` clause → rejected.
        assert!(validate_generated_plan_content("# Plan\n\n- [ ] do thing\n", &spec).is_err());
        // No spec criteria → rejected (would render an empty plan).
        let mut empty = sample_spec("01JABCDEF0123456789ABCDEFG");
        empty.criteria.clear();
        assert!(
            validate_generated_plan_content("# Plan\n\n- [ ] do thing\n  DONE WHEN: y\n", &empty)
                .is_err()
        );
        // All invariants satisfied → accepted.
        assert!(
            validate_generated_plan_content("# Plan\n\n- [ ] do thing\n  DONE WHEN: y\n", &spec)
                .is_ok()
        );
    }

    /// Round-8 (#1822) regression: the `DONE WHEN:` gate is PER-TASK, not a
    /// single global mention. Proves the two reviewer-named false-pass cases are
    /// rejected (a bare subject mention without a clause; a multi-task plan where
    /// only one open task carries a clause) while every genuinely-valid clause
    /// placement still passes.
    #[test]
    fn validate_generated_plan_content_requires_per_task_done_when() {
        let spec = sample_spec("01JABCDEF0123456789ABCDEFG");

        // REJECT: the only open task merely MENTIONS the keywords in its subject
        // with no `DONE WHEN:` clause (no colon, no criteria).
        assert!(
            validate_generated_plan_content("# Plan\n\n- [ ] Document DONE WHEN policy.\n", &spec)
                .is_err(),
            "a subject that only mentions the keywords (no clause) must be rejected"
        );

        // REJECT: multi-task plan where one open task has a clause but another
        // does not — the global check would wrongly pass on the first clause.
        let mixed =
            "# Plan\n\n## Tasks\n\n- [ ] Task one.\n  DONE WHEN: one is done.\n\n- [ ] Task two.\n";
        assert!(
            validate_generated_plan_content(mixed, &spec).is_err(),
            "any open task lacking a clause must reject the plan, even if a sibling has one"
        );

        // REJECT: a `DONE WHEN:` clause living on a COMPLETED task line must not
        // satisfy a sibling OPEN task that has no clause of its own.
        let clause_on_completed = "# Plan\n\n- [x] Done. DONE WHEN: only this completed line carries a clause.\n- [ ] Open task without its own clause.\n";
        assert!(
            validate_generated_plan_content(clause_on_completed, &spec).is_err(),
            "a clause on a completed task must not cover an open task's gap"
        );

        // ACCEPT: every open task has a clause on an indented FOLLOWING line.
        let following_line = "# Plan\n\n- [ ] Task one.\n  DONE WHEN: one is done.\n\n- [ ] Task two.\n  DONE WHEN: two is done.\n";
        assert!(
            validate_generated_plan_content(following_line, &spec).is_ok(),
            "following-line clauses on every open task must be accepted"
        );

        // ACCEPT: the clause sits on the checkbox SUBJECT line itself.
        assert!(
            validate_generated_plan_content(
                "# Plan\n\n- [ ] Implement X. DONE WHEN: tests pass.\n",
                &spec
            )
            .is_ok(),
            "a clause on the checkbox subject line must be accepted"
        );

        // ACCEPT: completed tasks without clauses are exempt; the lone open task
        // carries its own clause.
        let with_completed = "# Plan\n\n- [x] Already done, no clause needed.\n- [ ] Still open.\n  DONE WHEN: it is mechanically verifiable.\n";
        assert!(
            validate_generated_plan_content(with_completed, &spec).is_ok(),
            "completed tasks are exempt; only open tasks require a clause"
        );
    }

    /// Proves the publish closure runs INSIDE the held TODO write lock: a second,
    /// independent acquisition of the same lock must fail while the closure runs.
    /// flock(2) treats distinct open file descriptions as separate holders even
    /// within one process, so the non-blocking probe is denied while the outer
    /// lock is held. Linux-gated because the cross-fd flock semantics this relies
    /// on are guaranteed there.
    #[cfg(target_os = "linux")]
    #[test]
    fn persist_generated_plan_with_runs_publish_under_write_lock() {
        let dir = tempfile::tempdir().expect("tempdir");
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());
        let plan = manager
            .create("Lock scope probe", Some("fix/lock-scope-probe"))
            .expect("create plan");
        let spec = sample_spec(&plan.timestamp);
        let lock_path = manager.todos_dir().join(LOCK_FILE);
        let publish_ran = AtomicBool::new(false);

        let (result, lock_was_held) = manager
            .persist_generated_plan_with(
                &plan.timestamp,
                GeneratedPlanPersistRequest {
                    todo_content:
                        "# Lock scope probe\n\n## Tasks\n\n- [ ] Probe lock.\n  DONE WHEN: probed.\n",
                    spec: &spec,
                    epic_plan: None,
                },
                |res| {
                    publish_ran.store(true, Ordering::SeqCst);
                    // The file write must have already happened before publish.
                    let on_disk = std::fs::read_to_string(res.plan.todo_md_path())?;
                    assert!(
                        on_disk.contains("Lock scope probe"),
                        "publish must see the freshly written TODO content"
                    );
                    // A competing write-lock acquisition must be denied here.
                    let probe_file = std::fs::OpenOptions::new().write(true).open(&lock_path)?;
                    let mut probe = fd_lock::RwLock::new(probe_file);
                    Ok(probe.try_write().is_err())
                },
            )
            .expect("persist generated plan with publish");

        assert!(
            publish_ran.load(Ordering::SeqCst),
            "publish closure must be invoked"
        );
        assert!(
            lock_was_held,
            "the write lock must be held while the publish/commit step runs"
        );
        assert!(
            result.changed_files.iter().any(|f| f.ends_with("TODO.md")),
            "changed files must include the persisted TODO.md"
        );
    }

    /// Round-6 (#1822) regression: the TodoSave hook version for `csa todo
    /// persist` is computed INSIDE the held write lock (right after the commit)
    /// and returned from the publish step, so a concurrent TODO writer that
    /// commits another version AFTER the lock releases cannot change the version
    /// this save reports. Proven by capturing the publish-returned version, then
    /// committing a later version and showing a fresh recompute observes the
    /// bumped count while the captured under-lock value is unchanged. Linux-gated
    /// to match the sibling lock-probe test.
    #[cfg(target_os = "linux")]
    #[test]
    fn persist_generated_plan_with_returns_version_computed_under_lock() {
        let dir = tempfile::tempdir().expect("tempdir");
        let manager = TodoManager::with_base_dir(dir.path().to_path_buf());
        crate::git::ensure_git_init(manager.todos_dir()).expect("init todos git");
        let plan = manager
            .create("Version under lock", Some("fix/version-under-lock"))
            .expect("create plan");
        let spec = sample_spec(&plan.timestamp);
        // First committed version of TODO.md (the freshly created template).
        crate::git::save(manager.todos_dir(), &plan.timestamp, "create plan")
            .expect("save initial")
            .expect("initial commit");

        let todos_dir = manager.todos_dir().to_path_buf();
        let ts = plan.timestamp.clone();
        let (_result, captured_version) = manager
            .persist_generated_plan_with(
                &plan.timestamp,
                GeneratedPlanPersistRequest {
                    todo_content:
                        "# Version under lock\n\n## Tasks\n\n- [ ] Probe version.\n  DONE WHEN: probed.\n",
                    spec: &spec,
                    epic_plan: None,
                },
                // Mirror the production persist closure: commit, then count THIS
                // save's versions while the write lock is still held.
                |result| -> Result<usize> {
                    let files: Vec<&str> =
                        result.changed_files.iter().map(String::as_str).collect();
                    crate::git::save_files(&todos_dir, &ts, &files, "persist probe")?;
                    Ok(crate::git::list_versions(&todos_dir, &ts)?.len())
                },
            )
            .expect("persist with publish");

        // create + persist == the 2nd committed TODO.md version.
        assert_eq!(captured_version, 2, "version captured inside the held lock");

        // Simulate a concurrent writer winning the lock right after release and
        // committing another TODO.md version. A post-release recompute would now
        // observe 3; the captured under-lock value must remain 2.
        std::fs::write(
            plan.todo_md_path(),
            "# Version under lock\n\n## Tasks\n\n- [ ] Probe version again.\n  DONE WHEN: re-probed.\n",
        )
        .expect("write later version");
        crate::git::save(manager.todos_dir(), &plan.timestamp, "concurrent save")
            .expect("save later")
            .expect("later commit");
        let post_release_recompute =
            crate::git::list_versions(manager.todos_dir(), &plan.timestamp)
                .expect("list versions")
                .len();

        assert_eq!(
            post_release_recompute, 3,
            "a later concurrent save bumps a fresh recompute"
        );
        assert_ne!(
            captured_version, post_release_recompute,
            "the under-lock version must not equal a post-release recompute"
        );
    }
}
