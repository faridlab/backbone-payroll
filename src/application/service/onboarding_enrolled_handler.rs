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
        let row: Option<(Option<Decimal>,)> = sqlx::query_as(
            r#"SELECT base_salary
                 FROM employee.employees
                WHERE id = $1
                  AND (metadata->>'deleted_at') IS NULL"#,
        )
        .bind(employee_id)
        .fetch_optional(&self.pool)
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
        let company_id: Uuid = json_field(p, "company_id")?;
        let employee_id: Uuid = json_field(p, "employee_id")?;
        let onboarding_id: Option<Uuid> = serde_json::from_value(p["onboarding_id"].clone()).ok();

        // Read the starting salary BEFORE the write tx (a best-effort snapshot read on the pool, the
        // same pattern as lifecycle's PoolOffboardingInputs). None/0 → claim-but-skip.
        let base_salary = self.inputs.starting_salary(employee_id).await.map_err(map_db)?;

        let mut tx = self.pool.begin().await.map_err(map_db)?;

        // Claim the event in-tx with the effect: the inbox row + the (conditional) insert commit
        // together. A missing salary still claims (so a replay is a no-op) but skips the INSERT.
        let first_time = inbox::once(&mut *tx, "payroll", CONSUMER, event_id)
            .await
            .map_err(|e| handler_err(format!("inbox claim: {e}")))?;

        if first_time {
            if let Some(amount) = base_salary {
                // change_type='hire' is the initial-salary variant; reference_id = onboarding_id is the
                // non-null idempotency link back to the source workflow; effective_date = today (the
                // enrollment lands at completion time). `period = current year` (per ADR-005) is carried
                // in the note — compensation_changes carries an effective_date, not a period column.
                let period = Utc::now().format("%Y").to_string();
                let note = format!("onboarding enrollment: initial compensation (period {period})");
                sqlx::query(
                    r#"INSERT INTO payroll.compensation_changes
                           (company_id, employee_id, change_type, new_amount, effective_date,
                            reference_id, note)
                       VALUES ($1, $2, 'hire'::compensation_change_type, $3, $4, $5, $6)"#,
                )
                .bind(company_id)
                .bind(employee_id)
                .bind(amount)
                .bind(Utc::now().date_naive())
                .bind(onboarding_id)
                .bind(&note)
                .execute(&mut *tx)
                .await
                .map_err(map_db)?;
            }
            // else: no starting salary recorded yet — claim recorded, no row written (claim-but-skip).
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
