//! Consumer for the `promotion.effective` compound event — salary side (ADR-005).
//!
//! The payroll module owns the APPLY side of the promotion's salary change: on each
//! `promotion.effective` envelope it appends a `compensation_changes` row capturing the new salary,
//! **idempotently**. Registered on the integration bus in backbone-hr-app's `main.rs` alongside the
//! employee `PromotionEffectiveHandler` (both subscribe to `promotion.effective`; each dedups
//! independently via its own inbox consumer name).
//!
//! ## Null proposed_salary
//!
//! Not every promotion carries a salary change (a pure role/level move may have `proposed_salary =
//! null`). In that case this handler claims the event (so a replay does not retry) but skips the
//! INSERT — there is no compensation row to write.
//!
//! ## Idempotency
//!
//! The relay is at-least-once, so this handler MUST be idempotent. It uses [`backbone_outbox::inbox::once`]:
//! the `(consumer, event_id)` claim and the compensation_change INSERT run in ONE transaction and
//! commit together. `reference_id = promotion_id` is the non-null idempotency link back to the source
//! workflow.
//!
//! This is a user-owned custom file — it is NEVER regenerated.

use async_trait::async_trait;
use backbone_messaging::{EventError, IntegrationEventEnvelope, IntegrationEventHandler};
use backbone_outbox::inbox;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

/// The consumer name stamped into the payroll inbox. The ADR-005 idempotency key for this target is
/// `("promotion.salary", promotion_id)`; the `promotion_id` arrives as the envelope id (preserved
/// from the outbox row id).
const CONSUMER: &str = "promotion.salary";

/// Integration-event handler that turns a `promotion.effective` envelope into a `compensation_changes`
/// row, idempotently. Holds only the pool.
pub struct PromotionSalaryHandler {
    pool: PgPool,
}

impl PromotionSalaryHandler {
    /// Create a new handler bound to the given pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl IntegrationEventHandler for PromotionSalaryHandler {
    async fn handle(&self, envelope: IntegrationEventEnvelope) -> Result<(), EventError> {
        // The envelope id IS the outbox row's id (the relay preserves it) → the dedup key.
        let event_id = Uuid::parse_str(&envelope.id)
            .map_err(|e| handler_err(format!("bad envelope id '{}': {e}", envelope.id)))?;

        let p = &envelope.payload;
        let company_id: Uuid = json_field(p, "company_id")?;
        let employee_id: Uuid = json_field(p, "employee_id")?;
        let promotion_id: Option<Uuid> = serde_json::from_value(p["promotion_id"].clone()).ok();
        let effective_date: Option<NaiveDate> = serde_json::from_value(p["effective_date"].clone()).ok();
        // proposed_salary is carried as a JSON string (decimal → string for portability); parse it back.
        let proposed_salary: Option<Decimal> = serde_json::from_value::<String>(p["proposed_salary"].clone())
            .ok()
            .and_then(|s| s.parse().ok());

        let mut tx = self.pool.begin().await.map_err(map_db)?;

        // Claim the event in-tx with the effect: the inbox row + the (conditional) insert commit
        // together. A null proposed_salary still claims (so a replay is a no-op) but skips the INSERT.
        let first_time = inbox::once(&mut *tx, "payroll", CONSUMER, event_id)
            .await
            .map_err(|e| handler_err(format!("inbox claim: {e}")))?;

        if first_time {
            if let Some(amount) = proposed_salary {
                // change_type='promotion' for every promotion-driven comp change; reference_id =
                // promotion_id is the non-null idempotency link. effective_date carries the move's date.
                sqlx::query(
                    r#"INSERT INTO payroll.compensation_changes
                           (company_id, employee_id, change_type, new_amount, effective_date,
                            reference_id, note)
                       VALUES ($1, $2, 'promotion'::compensation_change_type, $3, $4, $5, $6)"#,
                )
                .bind(company_id)
                .bind(employee_id)
                .bind(amount)
                .bind(effective_date)
                .bind(promotion_id)
                .bind("promotion.effective")
                .execute(&mut *tx)
                .await
                .map_err(map_db)?;
            }
            // else: no salary component on this promotion — claim recorded, no row written.
        }

        tx.commit().await.map_err(map_db)?;
        Ok(())
    }

    fn event_patterns(&self) -> Vec<&'static str> {
        // Same pattern as PromotionEffectiveHandler — the bus dispatches one event to BOTH handlers;
        // each dedups via its own consumer name.
        vec!["promotion.effective"]
    }

    fn name(&self) -> &'static str {
        "PromotionSalaryHandler"
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
