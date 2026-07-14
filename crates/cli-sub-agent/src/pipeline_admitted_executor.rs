use std::ops::Deref;
use std::sync::atomic::{AtomicBool, Ordering};

use csa_core::model_catalog::{CatalogAdmission, CatalogWarning};
use csa_executor::{Executor, ModelSpec};

#[derive(Debug)]
pub(crate) struct AdmittedExecutor {
    executor: Executor,
    // Retained for upcoming csa-session ledger provenance consumers.
    #[allow(dead_code)]
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
        Self {
            executor,
            resolved_model_spec,
            catalog_warning: admission.warning().cloned(),
            warning_emitted: AtomicBool::new(false),
        }
    }

    /// Returns CSA's execution-boundary catalog-admitted identity.
    ///
    /// This immutable snapshot records the resolved tool, provider, model, and
    /// thinking budget selected for execution. It is not a provider-reported
    /// identity from a response.
    #[allow(dead_code)]
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
