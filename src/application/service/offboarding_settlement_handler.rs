//! Consumer for the `offboarding.closed` compound event — settlement side (ADR-005).
//!
//! The payroll module owns the APPLY side of the offboarding final settlement: on each
//! `offboarding.closed` envelope it appends a `compensation_changes` row carrying the settlement
//! total, **idempotently**. Registered on the integration bus in backbone-hr-app's
//! `main.rs` alongside the employee `OffboardingClosedHandler`.
//!
//! ## Producer-carried pesangon (no payroll→lifecycle edge)
//!
//! The settlement calc lives in `backbone-lifecycle`. To keep the dependency graph acyclic, payroll
//! does NOT recompute it — the producer (`OffboardingWriteService::close`) runs the calc and embeds
//! the breakdown in the event payload. This handler just reads the carried breakdown and writes
//! `compensation_changes` with `change_type='offboarding'`, `new_amount=breakdown.total` (the
//! settlement's net payable: severance items + unused-leave payout, without the last pay, which
//! the final payroll run pays), and a note naming every item so the row is self-describing.
//! Idempotent via `inbox::once` on the event id (preserved from the outbox row id through the
//! relay).
//!
//! Two payload generations are read. The itemised one (PP 35/2021) carries `uang_pesangon`,
//! `upmk`, `uang_pisah`, `unused_leave_payout`, `net_payable` and `legal_basis`, plus `pesangon`,
//! `upm` (always 0) and `total` for readers of the earlier shape. The earlier one carries only
//! `pesangon`, `upmk`, `upm`, `unused_leave_payout` and `total`. Either way the row's amount is
//! `total`.
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

/// The carried settlement breakdown, deserialized off the event payload. Payroll owns its own
/// mirror struct (it must NOT import `backbone-lifecycle`'s type — that would create a Cargo edge
/// and break the acyclic graph); the field names match the producer's payload. The itemised fields
/// are optional so an event from an earlier producer still reads.
#[derive(Debug, Clone, Deserialize)]
struct CarriedBreakdown {
    pesangon: Decimal,
    upmk: Decimal,
    #[serde(default)]
    upm: Decimal,
    unused_leave_payout: Decimal,
    total: Decimal,
    #[serde(default)]
    uang_pesangon: Option<Decimal>,
    #[serde(default)]
    uang_pisah: Option<Decimal>,
    #[serde(default)]
    legal_basis: Option<String>,
}

/// The settlement row's amount and note for a payload's breakdown (or its absence).
///
/// The amount is the carried `total`. A payload with no readable breakdown (an older producer)
/// records zero with a note flagged for manual review rather than poison the queue.
fn settlement_entry(breakdown: Option<&CarriedBreakdown>, reason: Option<&str>) -> (Decimal, String) {
    let reason_note = reason.map(|x| format!(" (reason={x})")).unwrap_or_default();
    match breakdown {
        Some(b) => match (b.uang_pesangon, b.uang_pisah) {
            // The itemised (PP 35/2021) payload.
            (Some(uang_pesangon), uang_pisah) => (
                b.total,
                format!(
                    "final settlement{reason_note}{}: uang_pesangon={} upmk={} uang_pisah={} unused_leave={} total={}",
                    b.legal_basis.as_deref().map(|l| format!(" [{l}]")).unwrap_or_default(),
                    uang_pesangon,
                    b.upmk,
                    uang_pisah.unwrap_or(Decimal::ZERO),
                    b.unused_leave_payout,
                    b.total,
                ),
            ),
            // The earlier payload.
            (None, _) => (
                b.total,
                format!(
                    "pesangon settlement{reason_note}: pesangon={} upmk={} upm={} unused_leave={} total={}",
                    b.pesangon, b.upmk, b.upm, b.unused_leave_payout, b.total,
                ),
            ),
        },
        None => (
            Decimal::ZERO,
            format!(
                "offboarding settlement: payload carried no pesangon_breakdown — manual review required{reason_note}"
            ),
        ),
    }
}

/// Integration-event handler that appends the real pesangon settlement row on `offboarding.closed`,
/// idempotently. Holds only the pool.
pub struct OffboardingSettlementHandler {
    pool: PgPool,
}

impl OffboardingSettlementHandler {
    /// The database this consumer writes on: the relay binds the tenant's
    /// pool as the request pool for the whole consumer call (ADR-0029 pool
    /// law); the composed pool is the fallback.
    fn rpool(&self) -> sqlx::PgPool {
        crate::request_pool::current().unwrap_or_else(|| self.pool.clone())
    }

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
        let employee_id: Uuid = json_field(p, "employee_id")?;
        let offboarding_id: Option<Uuid> = serde_json::from_value(p["offboarding_id"].clone()).ok();
        let last_working_day: Option<NaiveDate> = serde_json::from_value(p["last_working_day"].clone()).ok();
        let reason: Option<String> = serde_json::from_value(p["reason"].clone()).ok();

        // The producer carries the full pesangon breakdown. Parse it; if absent (legacy payload),
        // fall back to a zero-amount flagged row so the queue is never poisoned.
        let breakdown: Option<CarriedBreakdown> =
            serde_json::from_value(p["pesangon_breakdown"].clone()).ok();

        let mut tx = self.rpool().begin().await.map_err(map_db)?;

        // Tenancy (ADR-0029): the module is tenant-agnostic — relay the ambient org request scope
        // onto our own transaction so the INSERT passes the composing service's tenancy RLS fence.
        // A RELAY delivery carries no ambient scope, and the composing decorator's org-unit fill
        // reads one — without a scope the settlement INSERT dies on the fill's kind guard. Fall
        // back to the payload's owning company leg (the close knows whose tenant it is — fail
        // closed when the event names neither).
        let payload_company: Option<Uuid> = serde_json::from_value(p["company_id"].clone()).ok();
        let scope = backbone_orm::org_scope::current_org_scope().or_else(|| {
            payload_company.map(backbone_orm::org_scope::OrgScope::for_company_unit)
        });
        if let Some(scope) = scope {
            backbone_orm::org_scope::bind_org_scope_on(&mut tx, &scope)
                .await
                .map_err(|e| handler_err(format!("org scope bind: {e}")))?;
        }

        // Claim the event in-tx with the effect: the inbox row + the settlement insert commit together.
        let first_time = inbox::once(&mut *tx, "payroll", CONSUMER, event_id)
            .await
            .map_err(|e| handler_err(format!("inbox claim: {e}")))?;

        if first_time {
            let (amount, note) = settlement_entry(breakdown.as_ref(), reason.as_deref());

            // change_type='offboarding' is the dedicated enum variant for this; reference_id =
            // offboarding_id is the non-null idempotency link back to the source workflow.
            sqlx::query(
                r#"INSERT INTO payroll.compensation_changes (employee_id, change_type, new_amount, effective_date,
                        reference_id, note,
                            org_unit_id)
                       VALUES ($1, 'offboarding'::compensation_change_type, $2, $3, $4, $5, $6::uuid)"#,
            )
            .bind(employee_id)
            .bind(amount)
            .bind(last_working_day)
            .bind(offboarding_id)
            .bind(&note)
                .bind(backbone_orm::org_scope::current_org_scope().map(|s| s.acting_unit_id()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn parse(payload: serde_json::Value) -> Option<CarriedBreakdown> {
        serde_json::from_value(payload["pesangon_breakdown"].clone()).ok()
    }

    #[test]
    fn the_itemised_payload_records_its_total_and_names_the_items() {
        // The shape the PP 35/2021 producer emits (numbers as JSON numbers or strings).
        let p = serde_json::json!({ "pesangon_breakdown": {
            "severance_case": "resignation", "legal_basis": "PP 35/2021 Pasal 50",
            "uang_pesangon": "0", "upmk": "0", "uang_pisah": "8000000",
            "severance_total": "8000000", "unused_leave_days": "12",
            "unused_leave_payout": "4571428.56", "last_pay": "2666666.68",
            "last_pay_via_payroll": true, "net_payable": "12571428.56",
            "pesangon": "0", "upm": "0", "total": "12571428.56"
        }});
        let b = parse(p).expect("the itemised breakdown reads");
        let (amount, note) = settlement_entry(Some(&b), Some("resignation"));
        assert_eq!(amount, d("12571428.56"));
        assert!(note.contains("uang_pisah=8000000"), "{note}");
        assert!(note.contains("[PP 35/2021 Pasal 50]"), "{note}");
        assert!(!note.contains("upm="), "no UPM item under PP 35/2021: {note}");
    }

    #[test]
    fn the_earlier_payload_still_records_its_total() {
        let p = serde_json::json!({ "pesangon_breakdown": {
            "pesangon": 48000000.0, "upmk": 48000000.0, "upm": 14400000.0,
            "unused_leave_payout": 46909090.91, "total": 157309090.91
        }});
        let b = parse(p).expect("the earlier breakdown reads");
        let (amount, note) = settlement_entry(Some(&b), None);
        assert_eq!(amount, d("157309090.91"));
        assert!(note.starts_with("pesangon settlement: pesangon=48000000"), "{note}");
    }

    #[test]
    fn a_payload_without_a_breakdown_records_zero_for_review() {
        let b = parse(serde_json::json!({}));
        let (amount, note) = settlement_entry(b.as_ref(), Some("death"));
        assert_eq!(amount, Decimal::ZERO);
        assert!(note.contains("manual review required (reason=death)"), "{note}");
    }
}
