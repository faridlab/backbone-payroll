//! Consumer for the `recruitment.hired` compound event — the new employee's first salary.
//!
//! The payroll module owns the APPLY side of a hire's pay: on each `recruitment.hired` envelope it
//! appends the employee's initial `compensation_changes` row — the offered salary, effective on
//! the first day — **idempotently**. Without it a hired person has no current salary until their
//! onboarding completes, so payroll cannot pay them and an offboarding cannot compute their
//! severance.
//!
//! ## Which employee — a composition-time port
//!
//! The event names the offer, not the employee: the employee module's own consumer creates the
//! employee from it. Payroll takes no Cargo dependency on `backbone-employee`, so the composing
//! service supplies a [`HiredEmployeeResolver`] that answers, for an envelope, the id the employee
//! module gave (or will give) that hire. The handler then checks the employee row exists before
//! writing: registered after the employee consumer, it normally does; when it does not, the handler
//! refuses, rolling back its claim so the relay's re-emit retries it.
//!
//! ## The row
//!
//! `change_type = 'hire'` (the initial-salary variant), `new_amount` = the offer's
//! `proposed_salary`, `effective_date` = the first day (`start_date` when the event carries one,
//! else `join_date`), `reference_id` = the offer id. An offer without a salary claims and skips,
//! loudly — there is nothing to record.
//!
//! ## Idempotency and tenancy
//!
//! The `(consumer, event_id)` inbox claim and the INSERT commit in one transaction. The relay
//! delivers without an ambient org scope, so the transaction binds the payload's owning company
//! as the scope (an ambient scope wins when a caller bound one), the same posture as the employee
//! consumer of this event; a hire naming neither is refused.
//!
//! This is a user-owned custom file — it is NEVER regenerated.

use std::sync::Arc;

use async_trait::async_trait;
use backbone_messaging::{EventError, IntegrationEventEnvelope, IntegrationEventHandler};
use backbone_orm::org_scope::{self, OrgScope};
use backbone_outbox::inbox;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

/// The consumer name stamped into the payroll inbox, distinct from every other payroll consumer.
const CONSUMER: &str = "recruitment.hired.compensation";

/// Answers which employee a `recruitment.hired` envelope created. Implemented by the composing
/// service over the employee module's own derivation, so payroll stays free of that dependency.
pub trait HiredEmployeeResolver: Send + Sync {
    /// The employee id the hire created, or `None` when the envelope cannot name one.
    fn hired_employee_id(&self, envelope: &IntegrationEventEnvelope) -> Option<Uuid>;
}

/// Integration-event handler that records a hire's offered salary as the employee's first
/// compensation change.
pub struct HireCompensationHandler {
    pool: PgPool,
    resolver: Arc<dyn HiredEmployeeResolver>,
}

impl HireCompensationHandler {
    /// Create a handler bound to the given pool, resolving the hired employee through `resolver`.
    pub fn new(pool: PgPool, resolver: Arc<dyn HiredEmployeeResolver>) -> Self {
        Self { pool, resolver }
    }

    /// The database this consumer writes on: the relay binds the tenant's pool as the request
    /// pool for the whole consumer call (ADR-0029 pool law); the composed pool is the fallback.
    fn rpool(&self) -> PgPool {
        crate::request_pool::current().unwrap_or_else(|| self.pool.clone())
    }
}

/// The hire's first day: the proposed `start_date` when carried, else the `join_date`.
fn first_day(p: &serde_json::Value) -> Option<NaiveDate> {
    let date = |key: &str| {
        serde_json::from_value::<Option<NaiveDate>>(p[key].clone())
            .ok()
            .flatten()
    };
    date("start_date").or_else(|| date("join_date"))
}

#[async_trait]
impl IntegrationEventHandler for HireCompensationHandler {
    async fn handle(&self, envelope: IntegrationEventEnvelope) -> Result<(), EventError> {
        // The envelope id IS the outbox row's id (the relay preserves it) → the dedup key.
        let event_id = Uuid::parse_str(&envelope.id)
            .map_err(|e| handler_err(format!("bad envelope id '{}': {e}", envelope.id)))?;
        let p = &envelope.payload;

        let employee_id = self
            .resolver
            .hired_employee_id(&envelope)
            .ok_or_else(|| handler_err("the hire names no employee".into()))?;
        let offer_id: Option<Uuid> = serde_json::from_value(p["offer_id"].clone()).ok().flatten();
        // The offer's gross is carried as a decimal STRING (the producer serializes Numeric
        // through its Display).
        let salary: Option<Decimal> = serde_json::from_value::<Option<String>>(p["proposed_salary"].clone())
            .ok()
            .flatten()
            .and_then(|s| s.parse::<Decimal>().ok())
            .filter(|d| *d > Decimal::ZERO);
        let effective = first_day(p)
            .ok_or_else(|| handler_err("payload carries neither start_date nor join_date".into()))?;
        let company: Option<Uuid> = serde_json::from_value(p["company_id"].clone()).ok().flatten();

        let scope = org_scope::current_org_scope()
            .or_else(|| company.map(OrgScope::for_company_unit))
            .ok_or_else(|| {
                handler_err("no ambient org scope and no payload company_id — cannot place the row".into())
            })?;

        let mut tx = self.rpool().begin().await.map_err(map_db)?;
        org_scope::bind_org_scope_on(&mut tx, &scope)
            .await
            .map_err(|e| handler_err(format!("org scope bind: {e}")))?;

        let first_time = inbox::once(&mut *tx, "payroll", CONSUMER, event_id)
            .await
            .map_err(|e| handler_err(format!("inbox claim: {e}")))?;
        if !first_time {
            tx.commit().await.map_err(map_db)?;
            return Ok(());
        }

        let Some(amount) = salary else {
            // Claim-but-skip: an offer without a salary leaves nothing to record.
            tracing::warn!(
                target: "payroll.hire_compensation",
                employee_id = %employee_id,
                offer_id = ?offer_id,
                "hire compensation SKIPPED: the offer carries no salary — record the first \
                 compensation change by hand"
            );
            tx.commit().await.map_err(map_db)?;
            return Ok(());
        };

        // The employee consumer of this event runs first; until its row exists there is no one to
        // pay. Refusing rolls the claim back with everything else, so a re-emit retries.
        let employee_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM employee.employees WHERE id = $1)",
        )
        .bind(employee_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_db)?;
        if !employee_exists {
            return Err(handler_err(format!(
                "the hired employee {employee_id} does not exist yet"
            )));
        }

        sqlx::query(
            r#"INSERT INTO payroll.compensation_changes
                   (employee_id, change_type, new_amount, effective_date, reference_id, note, org_unit_id)
               VALUES ($1, 'hire'::compensation_change_type, $2, $3, $4, $5, $6)"#,
        )
        .bind(employee_id)
        .bind(amount)
        .bind(effective)
        .bind(offer_id)
        .bind("hire: the offered salary")
        .bind(scope.acting_unit_id())
        .execute(&mut *tx)
        .await
        .map_err(map_db)?;

        tx.commit().await.map_err(map_db)?;
        Ok(())
    }

    fn event_patterns(&self) -> Vec<&'static str> {
        vec!["recruitment.hired"]
    }

    fn name(&self) -> &'static str {
        "HireCompensationHandler"
    }
}

fn map_db(e: sqlx::Error) -> EventError {
    handler_err(format!("db: {e}"))
}

fn handler_err(message: String) -> EventError {
    EventError::handler(CONSUMER, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_first_day_prefers_the_proposed_start_date() {
        let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok();
        assert_eq!(
            first_day(&json!({ "start_date": "2026-11-02", "join_date": "2026-10-02" })),
            d("2026-11-02")
        );
        assert_eq!(
            first_day(&json!({ "start_date": null, "join_date": "2026-10-02" })),
            d("2026-10-02")
        );
        assert_eq!(first_day(&json!({})), None);
    }
}
