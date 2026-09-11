//! Payroll domain events (hand-authored, user-owned) — the public extension surface.
//!
//! Payroll is a GL producer: on posting a run it emits `PayrollPosted` (the salary journal landed) so
//! settlement can pay the net + remit the statutory payables, and reporting can reconcile. A consuming
//! service supplies the sink (bus, outbox, …).

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Why a sink could not accept an event. Publish failures are surfaced to the caller (the post
/// verb errors AFTER its row landed), because the remedy — re-run the post — is exactly the
/// at-least-once delivery path: the already-posted branch re-publishes, and consumers dedup by
/// record id. Swallowing the error here would silently lose the event instead.
#[derive(Debug, thiserror::Error)]
pub enum PayrollEventError {
    #[error("db: {0}")]
    Db(#[from] sqlx::Error),
    #[error("{0}")]
    Other(String),
}

/// One obligation the posted run created, owed to the account it credits. `statutory` routes it: true →
/// remit to a statutory authority (BPJS, PPh 21); false → an ordinary deduction (loan, advance). This is
/// the breakdown `backbone-payments` settles — without it the consumer would have to re-query payroll's
/// private slip-line tables (a boundary violation) to split the lump `total_deductions` per authority
/// (completeness council 2026-07-08).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PayrollPayable {
    pub gl_account_id: Uuid,
    pub amount: Decimal,
    pub statutory: bool,
}

/// A payroll run was posted to the GL — the salary journal exists; net pay + statutory payables are owed.
/// Carries the payable breakdown the settlement consumer needs: net pay clears `salary_payable_account_id`
/// (amount = `total_net`); each entry in `payables` is remitted to its own account.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PayrollPosted {
    pub payroll_entry_id: Uuid,
    /// The legacy tenancy twin (ADR-0029) — carried in the payload for unstripped consumers. The
    /// payroll tables hold no company column; the write path echoes the ambient org scope's legacy
    /// company id, or nil when none is bound. Nothing keys a statement on it.
    pub company_id: Uuid,
    pub journal_id: Uuid,
    pub post_id: Uuid,
    pub total_gross: Decimal,
    pub total_deductions: Decimal,
    pub total_net: Decimal,
    /// Net pay clears here (amount = `total_net`) — the employees' take-home payable.
    pub salary_payable_account_id: Uuid,
    /// Each deduction payable to remit, grouped by account (BPJS/PPh 21 payables + any other deduction).
    pub payables: Vec<PayrollPayable>,
}

/// The payroll domain-event union.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum PayrollEvent {
    PayrollPosted(PayrollPosted),
}

/// Sink the write path publishes to. A consuming service supplies its own (bus, durable outbox,
/// …). Async + fallible so a composition adapter can stage the event durably (e.g. into a
/// database outbox on its own connection) before acknowledging. Delivery is at-least-once:
/// re-running the post verb re-publishes, and consumers dedup by record id.
#[async_trait::async_trait]
pub trait PayrollEventSink: Send + Sync {
    async fn publish(&self, event: &PayrollEvent) -> Result<(), PayrollEventError>;
}

/// A no-op/logging sink for tests and single-process composition.
#[derive(Debug, Default, Clone)]
pub struct LoggingSink;

#[async_trait::async_trait]
impl PayrollEventSink for LoggingSink {
    async fn publish(&self, event: &PayrollEvent) -> Result<(), PayrollEventError> {
        tracing::info!(?event, "payroll event");
        Ok(())
    }
}
