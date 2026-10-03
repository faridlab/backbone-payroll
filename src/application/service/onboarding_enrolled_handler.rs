//! Consumer for the `onboarding.completed` compound event — initial compensation side (ADR-005).
//!
//! The payroll module owns the APPLY side of the onboarding enrollment: on each `onboarding.completed`
//! envelope it seeds the joiner's INITIAL `compensation_changes` row from their starting salary,
//! **idempotently**. Registered on the integration bus in backbone-hr-app's `main.rs` alongside the
//! employee `OnboardingCompletedHandler` (both subscribe to `onboarding.completed`; each dedups
//! independently via its own inbox consumer name).
//!
//! ## Starting-salary read — pool-backed port (acyclic graph)
//!
//! The starting salary lives on the employee master (`employee.employees.base_salary`). Payroll must
//! NOT take a Cargo dependency on `backbone-employee` — that would couple payroll to employee's
//! internal service API and risk a cycle. Instead payroll defines this [`OnboardingEnrollInputs`] trait
//! seam and ships a default [`PoolOnboardingEnrollInputs`] that does a scalar SQL read against
//! `employee.employees` (the same read pattern as lifecycle's `PoolOffboardingInputs`, just behind a
//! trait so it is injectable at composition time). The composer and the integration test use the
//! pool-backed default.
//!
//! ## Claim-but-skip on missing salary
//!
//! Not every joiner has a starting salary recorded at completion time (a pre-compensation hire, or the
//! `base_salary` column is still NULL). In that case this handler CLAIMS the event (so a replay does
//! not retry) but SKIPS the INSERT — there is no compensation row to write. This mirrors
//! `PromotionSalaryHandler`'s null-`proposed_salary` behaviour.
//!
//! ## change_type
//!
//! Uses the existing `'hire'` variant of `compensation_change_type` — that variant IS the
//! "initial salary on hire" semantic. (The ADR-005 TODO named an `'initial'` variant, but the enum
//! already covers it with `'hire'`; adding a synonym would only split the meaning.)
//!
//! ## Idempotency
//!
//! The relay is at-least-once, so this handler MUST be idempotent. It uses [`backbone_outbox::inbox::once`]:
//! the `(consumer, event_id)` claim and the compensation_change INSERT run in ONE transaction and
//! commit together. `reference_id = onboarding_id` is the non-null idempotency link back to the source
//! workflow.
//!
//! This is a user-owned custom file — it is NEVER regenerated.

use async_trait::async_trait;
use backbone_messaging::{EventError, IntegrationEventEnvelope, IntegrationEventHandler};
use backbone_outbox::inbox;
use chrono::Utc;
use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

/// The consumer name stamped into the payroll inbox. Scoped so this initial-compensation target is
/// distinct from the other `onboarding.completed` consumer (`onboarding.active` in employee). The
/// ADR-005 idempotency key for this target is `("onboarding.enroll", event_id)`; the `event_id` arrives
/// as the envelope id (preserved from the outbox row id through the relay).
const CONSUMER: &str = "onboarding.enroll";

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Salary read port — keeps payroll free of any `backbone-employee` Cargo edge.
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// The one cross-module input the onboarding-enrollment apply needs: the joiner's starting salary.
///
/// The default impl ([`PoolOnboardingEnrollInputs`]) does a scalar SQL read against
/// `employee.employees.base_salary`. Behind a trait so it is injectable/mockable at composition time;
/// the composer and the integration test use the pool-backed default.
#[async_trait]
pub trait OnboardingEnrollInputs: Send + Sync {
    /// The joiner's starting gross monthly salary from `employee.employees.base_salary`. `None` when
    /// the employee has no salary recorded yet (NULL or zero) — the handler treats that as
    /// claim-but-skip.
    async fn starting_salary(&self, employee_id: Uuid) -> Result<Option<Decimal>, sqlx::Error>;
}

/// Default pool-backed [`OnboardingEnrollInputs`] — a scalar SQL read against `employee.employees`.
/// Constructed from the shared pool the composer/test already holds. No `backbone-employee` Cargo dep:
/// the read is plain SQL, so the dependency graph stays acyclic.
pub struct PoolOnboardingEnrollInputs {
    pool: PgPool,
}

impl PoolOnboardingEnrollInputs {
    /// The database this consumer writes on: the relay binds the tenant's
    /// pool as the request pool for the whole consumer call (ADR-0029 pool
    /// law); the composed pool is the fallback.
    fn rpool(&self) -> sqlx::PgPool {
        crate::request_pool::current().unwrap_or_else(|| self.pool.clone())
    }

    /// Create a new pool-backed salary reader.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl OnboardingEnrollInputs for PoolOnboardingEnrollInputs {
    async fn starting_salary(&self, employee_id: Uuid) -> Result<Option<Decimal>, sqlx::Error> {
        // `base_salary` is NULL until HR records the joiner's starting salary. The read is scoped to
        // the latest non-deleted employee row (the metadata->>'deleted_at' audit column the framework
        // stamps). NULL/0 → None → the handler claims-but-skips.
        //
        // Tenancy (ADR-0029): both modules are tenant-agnostic — neither table carries a company
        // column, and org scoping is the composing service's tenancy RLS fence. The read still goes
        // through the legacy `company_scope::fetch_optional_scoped` twin for its connection
        // discipline: it rides the request-dedicated connection when the composing service bound
        // one, and the plain pool otherwise. The helper's legacy task-local branch is never taken —
        // this module sets no legacy scope of its own.
        let row: Option<(Option<Decimal>,)> = backbone_orm::company_scope::fetch_optional_scoped(
            &self.rpool(),
            sqlx::query_as(
                r#"SELECT base_salary
                     FROM employee.employees
                    WHERE id = $1
                      AND (metadata->>'deleted_at') IS NULL"#,
            )
            .bind(employee_id),
        )
        .await?;
        Ok(row
            .and_then(|(b,)| b)
            // A zero salary is treated as "not recorded" — same claim-but-skip path as NULL.
            .filter(|d| *d != Decimal::ZERO))
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The handler.
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// Integration-event handler that seeds the joiner's initial `compensation_changes` row on
/// `onboarding.completed`, idempotently.
///
/// Holds the write `pool` (for the inbox+insert transaction — same shape as `OffboardingSettlementHandler`)
/// AND a salary-read port (default: pool-backed). The salary read runs on the port; the write runs on
/// the pool's tx.
pub struct OnboardingEnrolledHandler {
    pool: PgPool,
    inputs: Box<dyn OnboardingEnrollInputs>,
}

impl OnboardingEnrolledHandler {
    /// The database this consumer writes on: the relay binds the tenant's
    /// pool as the request pool for the whole consumer call (ADR-0029 pool
    /// law); the composed pool is the fallback.
    fn rpool(&self) -> sqlx::PgPool {
        crate::request_pool::current().unwrap_or_else(|| self.pool.clone())
    }

    /// Create a new handler bound to the given pool, using the default pool-backed salary reader.
    /// The pool is cloned into both the write field and the default reader — `PgPool` is an `Arc`
    /// internally, so the clone is cheap.
    pub fn new(pool: PgPool) -> Self {
        Self {
            inputs: Box::new(PoolOnboardingEnrollInputs::new(pool.clone())),
            pool,
        }
    }
}

#[async_trait]
impl IntegrationEventHandler for OnboardingEnrolledHandler {
    async fn handle(&self, envelope: IntegrationEventEnvelope) -> Result<(), EventError> {
        // The envelope id IS the outbox row's id (the relay preserves it) → the dedup key.
        let event_id = Uuid::parse_str(&envelope.id)
            .map_err(|e| handler_err(format!("bad envelope id '{}': {e}", envelope.id)))?;

        let p = &envelope.payload;
        let employee_id: Uuid = json_field(p, "employee_id")?;
        let onboarding_id: Option<Uuid> = serde_json::from_value(p["onboarding_id"].clone()).ok();
        let company: Option<Uuid> = serde_json::from_value(p["company_id"].clone()).ok().flatten();

        // Tenancy (ADR-0029): the module is tenant-agnostic. A caller may have bound an org
        // request scope; a relay delivery has none, so the payload's owning company stands in —
        // without a scope the fenced employee read sees no row and the insert has no unit.
        let scope = backbone_orm::org_scope::current_org_scope()
            .or_else(|| company.map(backbone_orm::org_scope::OrgScope::for_company_unit));
        let pool = self.rpool();

        // Read the starting salary BEFORE the write tx (a best-effort snapshot read through the
        // port). None/0 → claim-but-skip. The read runs inside the scope above on the tenant's
        // pool, so the default pool-backed port rides that scoped request connection.
        let base_salary = match scope.clone() {
            Some(scope) => backbone_orm::org_scope::with_org_request_scope(
                &pool,
                scope,
                self.inputs.starting_salary(employee_id),
            )
            .await
            .map_err(map_db)?,
            None => self.inputs.starting_salary(employee_id).await,
        }
        .map_err(map_db)?;

        let mut tx = pool.begin().await.map_err(map_db)?;
        if let Some(scope) = &scope {
            backbone_orm::org_scope::bind_org_scope_on(&mut tx, scope)
                .await
                .map_err(|e| handler_err(format!("org scope bind: {e}")))?;
        }

        // Claim the event in-tx with the effect: the inbox row + the (conditional) insert commit
        // together. A missing salary still claims (so a replay is a no-op) but skips the INSERT.
        let first_time = inbox::once(&mut *tx, "payroll", CONSUMER, event_id)
            .await
            .map_err(|e| handler_err(format!("inbox claim: {e}")))?;

        // A joiner who already has a compensation history — a recruitment hire records the
        // offered salary when the hire lands — keeps it: completing the onboarding adds no second
        // initial row.
        let already_compensated: bool = if first_time {
            sqlx::query_scalar(
                r#"SELECT EXISTS (SELECT 1 FROM payroll.compensation_changes
                                   WHERE employee_id = $1 AND (metadata->>'deleted_at') IS NULL)"#,
            )
            .bind(employee_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_db)?
        } else {
            false
        };

        if first_time && !already_compensated {
            if let Some(amount) = base_salary {
                // change_type='hire' is the initial-salary variant; reference_id = onboarding_id is the
                // non-null idempotency link back to the source workflow; effective_date = today (the
                // enrollment lands at completion time). `period = current year` (per ADR-005) is carried
                // in the note — compensation_changes carries an effective_date, not a period column.
                let period = Utc::now().format("%Y").to_string();
                let note = format!("onboarding enrollment: initial compensation (period {period})");
                sqlx::query(
                    r#"INSERT INTO payroll.compensation_changes (employee_id, change_type, new_amount, effective_date,
                            reference_id, note,
                            org_unit_id)
                       VALUES ($1, 'hire'::compensation_change_type, $2, $3, $4, $5, $6::uuid)"#,
                )
                .bind(employee_id)
                .bind(amount)
                .bind(Utc::now().date_naive())
                .bind(onboarding_id)
                .bind(&note)
                .bind(scope.as_ref().map(|s| s.acting_unit_id()))
                .execute(&mut *tx)
                .await
                .map_err(map_db)?;
            }
            // else: no starting salary recorded yet — claim recorded, no row
            // written (claim-but-skip). A TYPED, VISIBLE skip: the joiner's
            // first compensation row is permanently absent for this
            // onboarding, so say so loudly with both ids an operator needs.
            // (A recruitment hire never lands here: its offered salary is
            // recorded when the hire lands, so this marks a joiner added
            // outside recruitment, or an offer with no salary.)
            else {
                tracing::warn!(
                    target: "payroll.onboarding_enrolled",
                    employee_id = ?employee_id,
                    onboarding_id = ?onboarding_id,
                    "onboarding enrollment SKIPPED: no starting salary on the employee master — \
                     record base_salary and add the initial compensation change by hand"
                );
            }
        }

        tx.commit().await.map_err(map_db)?;
        Ok(())
    }

    fn event_patterns(&self) -> Vec<&'static str> {
        // Same pattern as OnboardingCompletedHandler — the bus dispatches one event to BOTH handlers;
        // each dedups via its own consumer name.
        vec!["onboarding.completed"]
    }

    fn name(&self) -> &'static str {
        "OnboardingEnrolledHandler"
    }
}

fn json_field<T>(p: &serde_json::Value, field: &str) -> Result<T, EventError>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_value(p[field].clone())
        .map_err(|e| handler_err(format!("payload.{field}: {e}")))
}

fn map_db(e: sqlx::Error) -> EventError {
    handler_err(format!("db: {e}"))
}

fn handler_err(message: String) -> EventError {
    EventError::handler(CONSUMER, message)
}
