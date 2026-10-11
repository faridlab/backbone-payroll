//! The module's extension (hand-written; ADR-0031): what the module adds beside its
//! generated services. The generated module and builder carry `ModuleExt` and
//! `ModuleBuilderExt` and dereference to them, so their fields read as `module.field`.

#[allow(unused_imports)]
use super::*;

/// State the module adds; read through the generated module's `Deref`.
pub struct ModuleExt {
    // The validated write path: run lifecycle, computed slips, the balanced salary journal, and
    // the module-held fail-closed seams (GL / events / remittance default Unwired/Logging/Unwired).
    pub(crate) payroll_write_service: Arc<PayrollWriteService>,
}

/// State the builder adds.
pub struct ModuleBuilderExt {
}

impl Default for ModuleBuilderExt {
    fn default() -> Self {
        Self {
        }
    }
}

impl ModuleBuilderExt {
    /// Build the extension's state from what the generated build made.
    #[allow(unused_variables, clippy::redundant_clone)]
    pub(crate) fn build(self, parts: &ModuleParts<'_>) -> anyhow::Result<ModuleExt> {
        let db_pool = parts.db_pool.clone();
        // The validated write path over the same pool: pool-default input ports (works standalone),
        // seams fail-closed (Unwired GL / Logging events / Unwired remittance) — a composition root
        // swaps them via `with_payroll_write_service`.
        let payroll_write_service = Arc::new(PayrollWriteService::new(db_pool.clone()));
        Ok(ModuleExt {
            payroll_write_service,
        })
    }
}

impl crate::ThisModule {
    /// The module-held validated write service (run verbs, computed slips, seams).
    pub fn payroll_write_service(&self) -> Arc<PayrollWriteService> {
        self.payroll_write_service.clone()
    }
    /// Replace the write service at the composition root — the way a host wires real adapters over
    /// the fail-closed seams (e.g. staging domain events into an outbox) or swaps the pool-default
    /// input ports for adapters over the attendance/employee exports. Consuming `self` keeps the
    /// builder chain reading naturally:
    ///
    /// ```text
    /// let payroll = PayrollModule::builder()
    ///     .with_database(pool.clone())
    ///     .build()?
    ///     .with_payroll_write_service(Arc::new(
    ///         PayrollWriteService::new(pool.clone()).with_event_sink(my_outbox_sink),
    ///     ));
    /// ```
    ///
    /// Call before mounting routes; the module is consumed once at startup.
    pub fn with_payroll_write_service(mut self, svc: Arc<PayrollWriteService>) -> Self {
        self.payroll_write_service = svc;
        self
    }
}
