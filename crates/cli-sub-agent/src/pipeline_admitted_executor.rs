use std::ops::Deref;
use std::sync::atomic::{AtomicBool, Ordering};

use csa_core::model_catalog::{CatalogAdmission, CatalogWarning};
use csa_executor::{Executor, ModelSpec};

#[derive(Debug)]
pub(crate) struct AdmittedExecutor {
    executor: Executor,
    // Retained for upcoming csa-session ledger provenance consumers.
    resolved_model_spec: ModelSpec,
    catalog_warning: Option<CatalogWarning>,
    warning_emitted: AtomicBool,
}

impl AdmittedExecutor {
    pub(super) fn new(
        executor: Executor,
        resolved_model_spec: ModelSpec,
        admission: CatalogAdmission,
    ) -> Self {
        let admitted = Self {
            executor,
            resolved_model_spec,
            catalog_warning: admission.warning().cloned(),
            warning_emitted: AtomicBool::new(false),
        };
        debug_assert_eq!(
            admitted.resolved_model_spec().tool,
            admitted.executor.tool_name(),
            "catalog-admitted model identity must match the executor tool"
        );
        admitted
    }

    /// Returns CSA's execution-boundary catalog-admitted identity.
    ///
    /// This immutable snapshot records the resolved tool, provider, model, and
    /// thinking budget selected for execution. It is not a provider-reported
    /// identity from a response.
    pub(crate) fn resolved_model_spec(&self) -> &ModelSpec {
        &self.resolved_model_spec
    }

    /// Enable Codex fast-mode runtime metadata without changing model identity.
    pub(crate) fn enable_codex_fast_mode(&mut self) {
        self.executor.enable_codex_fast_mode();
    }

    #[cfg(test)]
    pub(crate) fn catalog_warning_pending(&self) -> bool {
        self.catalog_warning.is_some() && !self.warning_emitted.load(Ordering::Acquire)
    }
    /// Build the real admitted-executor type from the shipped closed catalog for
    /// production-adapter tests that must not reconstruct authority from config.
    #[cfg(test)]
    pub(crate) fn from_model_spec_for_test(model_spec: &str) -> anyhow::Result<Self> {
        let catalog = csa_core::model_catalog::EffectiveModelCatalog::shipped()?;
        let (resolved_model_spec, admission) = ModelSpec::parse_and_validate(
            model_spec,
            &catalog,
            &[
                "opencode",
                "codex",
                "claude-code",
                "openai-compat",
                "hermes",
            ],
        )?;
        let executor = Executor::from_spec(&resolved_model_spec)?;
        Ok(Self::new(executor, resolved_model_spec, admission))
    }

    #[cfg(test)]
    pub(crate) fn from_codex_model_spec_for_test(
        model_spec: &str,
        runtime_metadata: csa_executor::codex_runtime::CodexRuntimeMetadata,
    ) -> anyhow::Result<Self> {
        let catalog = csa_core::model_catalog::EffectiveModelCatalog::shipped()?;
        let (resolved_model_spec, admission) =
            ModelSpec::parse_and_validate(model_spec, &catalog, &["codex"])?;
        if resolved_model_spec.tool != "codex" {
            anyhow::bail!("test Codex executor requires a codex model spec");
        }
        let executor = Executor::Codex {
            model_override: Some(resolved_model_spec.model.clone()),
            thinking_budget: Some(resolved_model_spec.thinking_budget.clone()),
            runtime_metadata,
        };
        Ok(Self::new(executor, resolved_model_spec, admission))
    }
}

impl Deref for AdmittedExecutor {
    type Target = Executor;

    fn deref(&self) -> &Self::Target {
        &self.executor
    }
}

pub(crate) trait DispatchExecutor {
    fn executor(&self) -> &Executor;

    fn emit_catalog_warning(&self) {}
}

impl DispatchExecutor for Executor {
    fn executor(&self) -> &Executor {
        self
    }
}

impl DispatchExecutor for AdmittedExecutor {
    fn executor(&self) -> &Executor {
        &self.executor
    }

    fn emit_catalog_warning(&self) {
        if self.catalog_warning.is_some()
            && !self.warning_emitted.swap(true, Ordering::AcqRel)
            && let Some(warning) = self.catalog_warning.as_ref()
        {
            eprintln!("{warning}");
        }
    }
}
