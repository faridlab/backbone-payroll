//! Consumer for the `offboarding.closed` compound event — settlement side (ADR-005).
//!
//! The payroll module owns the APPLY side of the offboarding final settlement: on each
//! `offboarding.closed` envelope it appends a `compensation_changes` row carrying the REAL 🇮🇩
//! pesangon total, **idempotently**. Registered on the integration bus in backbone-hr-app's
//! `main.rs` alongside the employee `OffboardingClosedHandler`.
//!
//! ## Producer-carried pesangon (no payroll→lifecycle edge)
//!
//! The pesangon calc lives in `backbone-lifecycle`. To keep the dependency graph acyclic, payroll
//! does NOT recompute it — the producer (`OffboardingWriteService::close`) runs the calc and embeds
//! the full `PesangonBreakdown` in the event payload. This handler just reads the carried breakdown
//! and writes `compensation_changes` with `change_type='offboarding'`, `new_amount=breakdown.total`,
//! and a note carrying every component (pesangon / UPMK / UPM / unused-leave payout) so the row is
//! self-describing on a payslip. Idempotent via `inbox::once` on the event id (preserved from the
//! outbox row id through the relay).
//!
//! ## Legacy tolerance
//!
//! If a payload carries no `pesangon_breakdown` (an event emitted by an older producer), this
//! handler still commits — it writes `new_amount = 0` with a flagged note rather than poison the
//! queue. The current producer always carries the breakdown, so this is purely defensive.
//!
//! Timeoff balance encashment is a separate target and is intentionally NOT wired here.
//!
//! This is a user-owned custom file — it is NEVER regenerated.

use async_trait::async_trait;
use backbone_messaging::{EventError, IntegrationEventEnvelope, IntegrationEventHandler};
use backbone_outbox::inbox;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

/// The consumer name stamped into the payroll inbox. The ADR-005 idempotency key for this target is
/// `("offboarding.settlement", event_id)`; the `event_id` arrives as the envelope id (preserved from
/// the outbox row id through the relay).
const CONSUMER: &str = "offboarding.settlement";

/// The carried 🇮🇩 pesangon breakdown, deserialized off the event payload. Payroll owns its own
/// mirror struct (it must NOT import `backbone-lifecycle`'s type — that would create a Cargo edge
/// and break the acyclic graph); the field names match the producer's `PesangonBreakdown` exactly.
#[derive(Debug, Clone, Deserialize)]
struct CarriedBreakdown {
    pesangon: Decimal,
    upmk: Decimal,
    upm: Decimal,
    unused_leave_payout: Decimal,
    total: Decimal,
}

/// Integration-event handler that appends the real pesangon settlement row on `offboarding.closed`,
/// idempotently. Holds only the pool.
pub struct OffboardingSettlementHandler {
    pool: PgPool,
}

impl OffboardingSettlementHandler {
    /// Create a new handler bound to the given pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl IntegrationEventHandler for OffboardingSettlementHandler {
    async fn handle(&self, envelope: IntegrationEventEnvelope) -> Result<(), EventError> {
        // The envelope id IS the outbox row's id (the relay preserves it) → the dedup key.
        let event_id = Uuid::parse_str(&envelope.id)
            .map_err(|e| handler_err(format!("bad envelope id '{}': {e}", envelope.id)))?;

        let p = &envelope.payload;
        let company_id: Uuid = json_field(p, "company_id")?;
        let employee_id: Uuid = json_field(p, "employee_id")?;
        let offboarding_id: Option<Uuid> = serde_json::from_value(p["offboarding_id"].clone()).ok();
        let last_working_day: Option<NaiveDate> = serde_json::from_value(p["last_working_day"].clone()).ok();
        let reason: Option<String> = serde_json::from_value(p["reason"].clone()).ok();

        // The producer carries the full pesangon breakdown. Parse it; if absent (legacy payload),
        // fall back to a zero-amount flagged row so the queue is never poisoned.
        let breakdown: Option<CarriedBreakdown> =
            serde_json::from_value(p["pesangon_breakdown"].clone()).ok();

        let mut tx = self.pool.begin().await.map_err(map_db)?;

        // Claim the event in-tx with the effect: the inbox row + the settlement insert commit together.
        let first_time = inbox::once(&mut *tx, "payroll", CONSUMER, event_id)
            .await
            .map_err(|e| handler_err(format!("inbox claim: {e}")))?;

        if first_time {
            let (amount, note) = match (&breakdown, reason.as_deref()) {
                (Some(b), r) => {
                    let note = format!(
                        "pesangon settlement{}: pesangon={} upmk={} upm={} unused_leave={} total={}",
                        r.map(|x| format!(" (reason={x})")).unwrap_or_default(),
                        b.pesangon, b.upmk, b.upm, b.unused_leave_payout, b.total,
                    );
                    (b.total, note)
                }
                // Legacy payload (no breakdown): record a zero row flagged for manual review rather
                // than silently writing a wrong number or failing the message.
                (None, r) => {
                    let note = format!(
                        "offboarding settlement: payload carried no pesangon_breakdown — manual review required{}",
                        r.map(|x| format!(" (reason={x})")).unwrap_or_default(),
                    );
                    (Decimal::ZERO, note)
                }
            };

            // change_type='offboarding' is the dedicated enum variant for this; reference_id =
            // offboarding_id is the non-null idempotency link back to the source workflow.
            sqlx::query(
                r#"INSERT INTO payroll.compensation_changes
                       (company_id, employee_id, change_type, new_amount, effective_date,
                        reference_id, note)
                   VALUES ($1, $2, 'offboarding'::compensation_change_type, $3, $4, $5, $6)"#,
            )
            .bind(company_id)
            .bind(employee_id)
            .bind(amount)
            .bind(last_working_day)
            .bind(offboarding_id)
            .bind(&note)
            .execute(&mut *tx)
            .await
            .map_err(map_db)?;
        }

        tx.commit().await.map_err(map_db)?;
        Ok(())
    }

    fn event_patterns(&self) -> Vec<&'static str> {
        // Same pattern as OffboardingClosedHandler — the bus dispatches one event to BOTH handlers.
        vec!["offboarding.closed"]
    }

    fn name(&self) -> &'static str {
        "OffboardingSettlementHandler"
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
