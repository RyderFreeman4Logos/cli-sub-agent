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
    pub fn persist_generated_plan(
        &self,
        timestamp: &str,
        request: GeneratedPlanPersistRequest<'_>,
    ) -> Result<GeneratedPlanPersistResult> {
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

            Ok(GeneratedPlanPersistResult {
                plan,
                changed_files,
            })
        })
    }
}
