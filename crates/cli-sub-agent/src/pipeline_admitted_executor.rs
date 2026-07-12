use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, Ordering};

use csa_core::model_catalog::{CatalogAdmission, CatalogWarning};
use csa_executor::Executor;

#[derive(Debug)]
pub(crate) struct AdmittedExecutor {
    executor: Executor,
    catalog_warning: Option<CatalogWarning>,
    warning_emitted: AtomicBool,
}

impl AdmittedExecutor {
    pub(super) fn new(executor: Executor, admission: Option<CatalogAdmission>) -> Self {
        Self {
            executor,
            catalog_warning: admission
                .as_ref()
                .and_then(CatalogAdmission::warning)
                .cloned(),
            warning_emitted: AtomicBool::new(false),
        }
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

impl DerefMut for AdmittedExecutor {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.executor
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
