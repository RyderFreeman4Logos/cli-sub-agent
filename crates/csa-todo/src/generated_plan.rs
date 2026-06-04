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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LOCK_FILE;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn sample_spec(plan_ulid: &str) -> SpecDocument {
        SpecDocument {
            schema_version: 1,
            plan_ulid: plan_ulid.to_string(),
            summary: "lock-scope probe".to_string(),
            criteria: Vec::new(),
        }
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
}
